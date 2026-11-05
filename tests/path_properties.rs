//! Discovery-only path grammar, representation, HOME-history and filesystem probes.
//! Run `cargo test --test path_properties -- --nocapture` for the broad suite.
//! PATH_PROP_CHECK=semantic selects the filesystem contract property (known failure).
//! PATH_PROP_CHECK=raw selects strict byte preservation (a deliberately stronger,
//! potentially ambiguous lexical contract). Both retain shrunk failures separately.
#![cfg(unix)]

use expanduser::{ExpandUser, ExpandUserError};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, TestCaseError, TestRunner};
use std::collections::BTreeMap;
use std::ffi::{CStr, OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

// No Path::components, Path::join or production helpers: expansion is a byte
// grammar rewrite, restricted to current-user and non-expanding expressions.
fn raw_oracle(input: &[u8], home: &[u8]) -> Vec<u8> {
    if input == b"~" || input.starts_with(b"~/") {
        let mut out = home.to_vec();
        out.extend_from_slice(&input[1..]);
        out
    } else {
        input.to_vec()
    }
}

// Weak, explicitly lexical quotient used only for the broad grammar checks.
// It does NOT claim dots/trailing separators preserve filesystem semantics.
fn lexical_components(input: &[u8]) -> (bool, Vec<Vec<u8>>) {
    (
        input.first() == Some(&b'/'),
        input.split(|b| *b == b'/')
            .filter(|p| !p.is_empty() && *p != b".")
            .map(<[u8]>::to_vec)
            .collect(),
    )
}

fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

fn expand(input: &[u8]) -> Result<Vec<u8>, ExpandUserError> {
    OsStr::from_bytes(input).expand_user().map(|p| bytes(&p).to_vec())
}

fn representations(input: &[u8]) -> Result<Vec<u8>, TestCaseError> {
    let os = OsString::from_vec(input.to_vec());
    let owned_path = PathBuf::from(&os);
    let expected = expand(input).map_err(|e| TestCaseError::fail(format!("{input:?}: {e}")))?;
    for result in [os.expand_user(), owned_path.expand_user(), owned_path.as_path().expand_user()] {
        let actual = result.map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(bytes(&actual), expected.as_slice());
    }
    if let Ok(text) = std::str::from_utf8(input) {
        let from_str = text.expand_user().unwrap();
        let from_string = text.to_string().expand_user().unwrap();
        prop_assert_eq!(bytes(&from_str), expected.as_slice());
        prop_assert_eq!(bytes(&from_string), expected.as_slice());
    }
    Ok(expected)
}

fn atoms() -> Vec<Vec<u8>> {
    [b"".as_slice(), b".", b"..", b"x", b"file", b"~", b"~other", b"a\0b", b"\xff", b"~\xff", "é".as_bytes(), "e\u{301}".as_bytes(), "🦀".as_bytes()]
        .into_iter().map(<[u8]>::to_vec).collect()
}

fn path_strategy() -> impl Strategy<Value = Vec<u8>> {
    // Every first component is either '~', non-tilde, absolute, dot/parent,
    // or invalid UTF-8. Named-account resolution is deliberately excluded.
    let first = prop::sample::select(vec![b"".to_vec(), b"~".to_vec(), b"x".to_vec(), b".".to_vec(), b"..".to_vec(), b"\xff".to_vec(), b"~\xff".to_vec(), "é".as_bytes().to_vec()]);
    let grammar = (any::<bool>(), first, prop::collection::vec((1usize..5, prop::sample::select(atoms())), 0..8), 0usize..5)
        .prop_map(|(absolute, first, tail, trailing)| {
            let mut value = if absolute { b"/".to_vec() } else { Vec::new() };
            value.extend(first);
            for (separators, part) in tail {
                value.extend(std::iter::repeat(b'/').take(separators));
                value.extend(part);
            }
            value.extend(std::iter::repeat(b'/').take(trailing));
            value
        });
    let opaque = (prop::sample::select(vec![b"x".to_vec(), b"~/".to_vec(), b"/".to_vec(), b"./".to_vec()]), prop::collection::vec(any::<u8>(), 0..2048))
        .prop_map(|(mut prefix, tail)| { prefix.extend(tail); prefix });
    prop_oneof![5 => grammar, 1 => opaque]
}

fn homes() -> Vec<Vec<u8>> {
    [b"/home/one".as_slice(), b"/home/two/", b"/", b"relative", b".", b"..", b"/a//./b/../c", b"/nonutf8-\xff", b"~", "相対/家".as_bytes()]
        .into_iter().map(<[u8]>::to_vec).collect()
}

#[derive(Clone, Debug)]
enum Op {
    Set(Vec<u8>),
    Unset,
    Empty,
    Query(Vec<u8>),
}

fn histories() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(prop_oneof![3 => prop::sample::select(homes()).prop_map(Op::Set), 1 => Just(Op::Unset), 1 => Just(Op::Empty), 6 => path_strategy().prop_map(Op::Query)], 1..60)
}

// Independent system query for HOME-unset/empty expectations, with ERANGE retry.
// No named-user lookup is performed. This is a provider oracle, not a claim that
// the host account database is correct or that missing-account paths were tested.
fn account_home() -> Option<Vec<u8>> {
    let mut size = 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let code = unsafe { libc::getpwuid_r(libc::getuid(), &mut passwd, buffer.as_mut_ptr().cast(), size, &mut result) };
        if code == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if code != 0 || result.is_null() || passwd.pw_dir.is_null() { return None; }
        let home = unsafe { CStr::from_ptr(passwd.pw_dir) }.to_bytes();
        return (!home.is_empty()).then(|| home.to_vec());
    }
}

fn runner(mode: &str) -> TestRunner {
    let regression = match mode {
        "semantic" => "tests/path_semantic.proptest-regressions",
        "raw" => "tests/path_raw.proptest-regressions",
        _ => "tests/path_core.proptest-regressions",
    };
    TestRunner::new(Config {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(4096),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(regression))),
        source_file: Some(file!()),
        ..Config::default()
    })
}

fn count(coverage: &mut BTreeMap<&'static str, usize>, key: &'static str) {
    *coverage.entry(key).or_default() += 1;
}

fn check(input: &[u8], home: &[u8], coverage: &mut BTreeMap<&'static str, usize>) -> Result<(), TestCaseError> {
    let actual = representations(input)?;
    let expected = raw_oracle(input, home);
    prop_assert_eq!(lexical_components(&actual), lexical_components(&expected), "input={:?}, home={:?}", input, home);
    count(coverage, "queries");
    if actual != expected { count(coverage, "byte_discrepancies_not_assumed_bugs"); }
    if input == b"~" || input.starts_with(b"~/") { count(coverage, "current_home_expansion"); }
    else { count(coverage, "nonexpanding"); }
    if input.windows(2).any(|v| v == b"//") { count(coverage, "repeated_separator"); }
    if input.ends_with(b"/") { count(coverage, "trailing_separator"); }
    if input.split(|b| *b == b'/').any(|v| v == b".") { count(coverage, "dot"); }
    if input.split(|b| *b == b'/').any(|v| v == b"..") { count(coverage, "parent"); }
    if std::str::from_utf8(input).is_err() { count(coverage, "non_utf8"); }
    else if !input.is_ascii() { count(coverage, "unicode"); }
    if input.contains(&0) { count(coverage, "embedded_nul"); }
    Ok(())
}

fn core() {
    let fallback = account_home();
    let mut coverage = BTreeMap::new();
    // Explicit Cartesian coverage ensures rare HOME/grammar pairs are reached.
    let prefixes: &[&[u8]] = &[b"", b"~", b".", b"..", b"/", b"x", b"\xff", b"~\xff"];
    let separators: &[&[u8]] = &[b"/", b"//", b"///"];
    for home in homes() {
        std::env::set_var("HOME", OsStr::from_bytes(&home));
        for first in prefixes {
            check(first, &home, &mut coverage).unwrap();
            for sep in separators {
                for atom in atoms() {
                    for trailing in [b"".as_slice(), b"/", b"/.", b"//"] {
                        let input = [*first, *sep, atom.as_slice(), trailing].concat();
                        check(&input, &home, &mut coverage).unwrap();
                    }
                }
            }
        }
    }
    println!("exhaustive Cartesian coverage: {coverage:?}");
    let coverage = std::cell::RefCell::new(coverage);
    runner("core").run(&histories(), |ops| {
        let mut home = b"/initial-home".to_vec();
        std::env::set_var("HOME", OsStr::from_bytes(&home));
        for op in ops {
            match op {
                Op::Set(value) => {
                    std::env::set_var("HOME", OsStr::from_bytes(&value));
                    home = value;
                    count(&mut coverage.borrow_mut(), "set_home");
                }
                Op::Unset | Op::Empty => {
                    if matches!(op, Op::Unset) {
                        std::env::remove_var("HOME");
                        count(&mut coverage.borrow_mut(), "unset_home");
                    } else {
                        std::env::set_var("HOME", "");
                        count(&mut coverage.borrow_mut(), "empty_home");
                    }
                    if let Some(value) = &fallback { home = value.clone(); }
                    else {
                        prop_assert_eq!(OsStr::new("~").expand_user(), Err(ExpandUserError::CurrentUserHomeNotFound));
                        // Query assertions below need an actual home. Restore a
                        // known value only on hosts with no account fallback.
                        home = b"/no-account-fallback".to_vec();
                        std::env::set_var("HOME", OsStr::from_bytes(&home));
                    }
                }
                Op::Query(input) => check(&input, &home, &mut coverage.borrow_mut())?,
            }
            // Warm → mutate → query is guaranteed at every transition, not
            // dependent on random query placement. Also query suffix/parent.
            check(b"~", &home, &mut coverage.borrow_mut())?;
            check(b"~/./x/../\xff/", &home, &mut coverage.borrow_mut())?;
        }
        Ok(())
    }).unwrap();
    println!("total grammar and HOME-history coverage: {:?}; fallback_available={}", coverage.borrow(), fallback.is_some());
    sensitivity_calibration();
    semantic_probe(false);
}

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Entry { dev: u64, ino: u64, is_dir: bool },
    Error(Option<i32>),
}

fn observe(path: &[u8]) -> Observation {
    match fs::metadata(OsStr::from_bytes(path)) {
        Ok(meta) => Observation::Entry { dev: meta.dev(), ino: meta.ino(), is_dir: meta.is_dir() },
        Err(error) => Observation::Error(error.raw_os_error()),
    }
}

fn semantic_probe(strict: bool) {
    let fixture = tempfile::tempdir().unwrap();
    fs::write(fixture.path().join("file"), b"payload").unwrap();
    fs::create_dir(fixture.path().join("dir")).unwrap();
    fs::write(fixture.path().join("dir/child"), b"child").unwrap();
    symlink("file", fixture.path().join("file-link")).unwrap();
    symlink("dir", fixture.path().join("dir-link")).unwrap();
    let home = bytes(fixture.path()).to_vec();
    std::env::set_var("HOME", OsStr::from_bytes(&home));
    // cwd changes, like HOME mutations, are confined to this dedicated child.
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(fixture.path()).unwrap();
    let mut relative_differences = 0;
    for input in [b"file".as_slice(), b"file/", b"file/.", b"file-link", b"./file", b"./file/", b"dir/../file"] {
        let actual = expand(input).unwrap();
        relative_differences += usize::from(observe(input) != observe(&actual));
        println!("relative witness input={:?}, actual={:?}, expected={:?}, observed={:?}",
            OsStr::from_bytes(input), OsStr::from_bytes(&actual), observe(input), observe(&actual));
    }
    std::env::set_current_dir(original_cwd).unwrap();
    let stems: &[&[u8]] = &[b"file", b"file-link", b"dir", b"dir-link", b"dir/child", b"missing", b"dir-link/../file", b"dir/../file", b"file/..", b".", b"..", b"", b"file/.", b"file/./..", b"dir/./child", b"dir-link/./child"];
    let tails: &[&[u8]] = &[b"", b"/", b"//", b"/.", b"/./", b"//.", b"/..", b"/../"];
    let mut differences = 0;
    let mut total = 0;
    for stem in stems {
        for tail in tails {
            for prefix in [b"~".as_slice(), home.as_slice()] {
                let input = [prefix, b"/", *stem, *tail].concat();
                let expected = raw_oracle(&input, &home);
                let actual = expand(&input).unwrap();
                total += 1;
                if observe(&expected) != observe(&actual) {
                    differences += 1;
                    if differences <= 6 {
                        println!("filesystem mismatch input={:?}, expected={:?}, actual={:?}, output={:?}",
                            OsStr::from_bytes(&input), observe(&expected), observe(&actual), OsStr::from_bytes(&actual));
                    }
                }
            }
        }
    }
    // A HOME that is a regular file is still a valid value in the public API;
    // converting bare '~' into a directory-only path must not be overlooked.
    let file = [home.as_slice(), b"/file"].concat();
    std::env::set_var("HOME", OsStr::from_bytes(&file));
    let actual = expand(b"~").unwrap();
    let home_file_difference = observe(&file) != observe(&actual);
    println!("HOME=file witness expected={:?}; actual={:?}; output={:?}", observe(&file), observe(&actual), OsStr::from_bytes(&actual));
    println!("filesystem Cartesian coverage: cases={total}, semantic_differences={differences}");
    std::env::set_var("HOME", OsStr::from_bytes(&home));
    if strict {
        // Shrinks the representation and suffix independently. Each input has
        // a real file fixture, so this oracle cannot hide a meaningful slash.
        runner("semantic").run(&(0usize..2, 0usize..tails.len(), any::<bool>()), |(stem, tail, tilde)| {
            let prefix = if tilde { b"~".as_slice() } else { home.as_slice() };
            let input = [prefix, b"/", stems[stem], tails[tail]].concat();
            let expected = raw_oracle(&input, &home);
            let actual = expand(&input).unwrap();
            prop_assert_eq!(observe(&actual), observe(&expected), "input={:?}, actual={:?}, expected={:?}", input, actual, expected);
            Ok(())
        }).unwrap();
        assert_eq!(differences, 0, "exhaustive filesystem suffix matrix");
        assert_eq!(relative_differences, 0, "relative and single-component filesystem witnesses");
        assert!(!home_file_difference, "bare tilde must not impose directory-only semantics");
    }
}

fn sensitivity_calibration() {
    // Plausible mutant: expand every '~' component, not only the first one.
    // This is deliberately an oracle-level mutant, not a production edit.
    let input = b"x/~/y";
    let home = b"/home/calibration";
    let mutant = b"x/home/calibration/y";
    assert_ne!(lexical_components(mutant), lexical_components(&raw_oracle(input, home)));
    // A byte oracle sees a spurious trailing slash while Path equality does not;
    // the filesystem suite separately determines whether it matters.
    assert_eq!(Path::new("file"), Path::new("file/"));
    assert_ne!(b"file".as_slice(), b"file/".as_slice());
    println!("calibration: all-component tilde mutant rejected; trailing-slash byte mutant rejected");
}

#[test]
fn path_properties() {
    if std::env::var_os("EXPANDUSER_PATH_CHILD").is_none() {
        // All environment mutation is isolated to this dedicated binary child;
        // cargo's other integration/unit test binaries never share its HOME.
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "path_properties", "--nocapture", "--test-threads=1"])
            .env("EXPANDUSER_PATH_CHILD", "1")
            .status().unwrap();
        assert!(status.success(), "isolated path-property subprocess failed: {status}");
        return;
    }
    match std::env::var("PATH_PROP_CHECK").as_deref() {
        Ok("semantic") => semantic_probe(true),
        Ok("raw") => {
            std::env::set_var("HOME", "/home/raw");
            runner("raw").run(&path_strategy(), |input| {
                prop_assert_eq!(representations(&input)?, raw_oracle(&input, b"/home/raw"));
                Ok(())
            }).unwrap();
        }
        _ => core(),
    }
}
