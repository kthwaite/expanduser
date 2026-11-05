//! Local discovery suite. Run with `cargo test --test account_properties -- --nocapture`.
//! On the baseline, root-oracle checks fail when root's home is not /root.
//! Shared-storage stress is opt-in; it is not a race detector and a passing run
//! establishes neither Linux thread safety nor freedom from foreign-call interference.
#![cfg(unix)]

use expanduser::{ExpandUser, ExpandUserError};
use proptest::prelude::*;
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex},
};

static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
struct Account {
    name: String,
    home: PathBuf,
    uid: libc::uid_t,
}

// Independent OS oracle: caller-owned storage, no production helper or getpwnam.
fn lookup(name: Option<&str>) -> Option<Account> {
    let name = name.map(|s| CString::new(s).expect("oracle name contains NUL"));
    let mut size = 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let code = unsafe {
            match &name {
                Some(name) => libc::getpwnam_r(
                    name.as_ptr(),
                    pwd.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
                None => libc::getpwuid_r(
                    libc::getuid(),
                    pwd.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                ),
            }
        };
        if code == libc::ERANGE {
            size = size.checked_mul(2).expect("oracle buffer overflow");
            assert!(
                size <= 16 * 1024 * 1024,
                "oracle account record exceeds 16 MiB"
            );
            continue;
        }
        assert_eq!(code, 0, "account oracle failed: {:?}", name);
        if result.is_null() {
            return None;
        }
        let pwd = unsafe { pwd.assume_init() };
        assert!(!pwd.pw_name.is_null() && !pwd.pw_dir.is_null());
        return Some(Account {
            name: unsafe { CStr::from_ptr(pwd.pw_name) }
                .to_str()
                .expect("non-UTF8 account name")
                .to_owned(),
            home: PathBuf::from(OsStr::from_bytes(
                unsafe { CStr::from_ptr(pwd.pw_dir) }.to_bytes(),
            )),
            uid: pwd.pw_uid,
        });
    }
}

fn accounts() -> &'static Vec<Account> {
    static ACCOUNTS: LazyLock<Vec<Account>> = LazyLock::new(|| {
        let mut names = vec![
            "root".to_owned(),
            lookup(None).expect("current account absent").name,
        ];
        #[cfg(target_os = "macos")]
        let command = Command::new("dscl").args([".", "-list", "/Users"]).output();
        #[cfg(not(target_os = "macos"))]
        let command = Command::new("getent").arg("passwd").output();
        if let Ok(output) = command {
            if output.status.success() {
                for line in String::from_utf8(output.stdout)
                    .expect("account inventory is not UTF8")
                    .lines()
                {
                    names.push(line.split(':').next().unwrap().to_owned());
                }
            }
        }
        names.extend(["daemon", "nobody", "bin", "sys", "_www"].map(str::to_owned));
        names.sort();
        names.dedup();
        let accounts: Vec<_> = names.iter().filter_map(|name| lookup(Some(name))).collect();
        eprintln!(
            "account oracle inventory: {} accounts; current={:?}; root={:?}",
            accounts.len(),
            lookup(None),
            lookup(Some("root"))
        );
        assert!(accounts.iter().any(|a| a.name == "root"));
        assert!(accounts.iter().any(|a| a.name != "root"));
        accounts
    });
    &ACCOUNTS
}

fn suffix() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9_-]{1,24}",
            Just("документы".into()),
            Just("日本語".into()),
            Just("e\u{301}".into()),
            Just("~root".into()),
            Just("..".into()),
            Just(".hidden".into())
        ],
        0..6,
    )
}

fn input_for(name: &str, suffix: &[String]) -> String {
    let mut input = format!("~{name}");
    for part in suffix {
        input.push('/');
        input.push_str(part);
    }
    input
}

fn expected_for(account: &Account, suffix: &[String]) -> PathBuf {
    let mut expected = account.home.clone();
    for part in suffix {
        expected.push(part);
    }
    expected
}

fn expand_as(input: &str, representation: u8) -> Result<PathBuf, ExpandUserError> {
    match representation % 6 {
        0 => input.expand_user(),
        1 => input.to_owned().expand_user(),
        2 => Path::new(input).expand_user(),
        3 => PathBuf::from(input).expand_user(),
        4 => OsStr::new(input).expand_user(),
        _ => OsString::from(input).expand_user(),
    }
}

#[test]
fn root_home_matches_account_database() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let root = lookup(Some("root")).expect("root account absent");
    for suffix in [
        vec![],
        vec![".bashrc".into()],
        vec!["日本語".into(), "..".into(), "file".into()],
    ] {
        for representation in 0..6 {
            assert_eq!(
                expand_as(&input_for("root", &suffix), representation),
                Ok(expected_for(&root, &suffix)),
                "representation={representation}, suffix={suffix:?}"
            );
        }
    }
}

#[test]
fn every_existing_nonroot_account_and_representation() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for account in accounts().iter().filter(|a| a.name != "root") {
        for suffix in [
            vec![],
            vec!["file".into()],
            vec!["日本語".into(), "..".into(), "~root".into()],
        ] {
            for representation in 0..6 {
                assert_eq!(
                    expand_as(&input_for(&account.name, &suffix), representation),
                    Ok(expected_for(account, &suffix)),
                    "account={account:?}, representation={representation}"
                );
            }
        }
    }
}

#[test]
fn unknown_unicode_and_embedded_nul_are_not_existing_prefixes() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for name in [
        "expanduser_no_such_account_6e31",
        "юзер_6e31",
        "日本語_6e31",
        "é_6e31",
    ] {
        assert!(
            lookup(Some(name)).is_none(),
            "fixture unexpectedly exists: {name}"
        );
        for representation in 0..6 {
            for suffix in ["", "/file", "/日本語"] {
                assert_eq!(
                    expand_as(&format!("~{name}{suffix}"), representation),
                    Err(ExpandUserError::UserNotFound { user: name.into() })
                );
            }
        }
    }
    for prefix in ["root", &lookup(None).unwrap().name] {
        for tail in ["", "x", "日本語"] {
            let name = format!("{prefix}\0{tail}");
            for representation in 0..6 {
                assert_eq!(
                    expand_as(&format!("~{name}/file"), representation),
                    Err(ExpandUserError::InvalidTildeExpression { expr: name.clone() })
                );
            }
        }
    }
}

#[test]
fn non_utf8_and_nul_suffixes_are_path_data() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let account = accounts().iter().find(|a| a.name != "root").unwrap();
    for suffix in [b"file\xff".as_slice(), b"file\0name".as_slice()] {
        let mut bytes = format!("~{}/", account.name).into_bytes();
        bytes.extend_from_slice(suffix);
        let input = OsString::from_vec(bytes);
        let expected = account.home.join(OsStr::from_bytes(suffix));
        assert_eq!(
            input.expand_user().unwrap().as_os_str().as_bytes(),
            expected.as_os_str().as_bytes()
        );
        assert_eq!(
            Path::new(&input)
                .expand_user()
                .unwrap()
                .as_os_str()
                .as_bytes(),
            expected.as_os_str().as_bytes()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        max_shrink_iters: 4096,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct(
            "tests/account_properties.proptest-regressions",
        ))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn named_accounts_match_reentrant_oracle(force_root in any::<bool>(), index in 0usize..1024, suffix in suffix(), representation in 0u8..6) {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Root gets explicit weight rather than vanishing among system accounts.
        let account = if force_root { lookup(Some("root")).unwrap() }
            else { accounts()[index % accounts().len()].clone() };
        prop_assert_eq!(expand_as(&input_for(&account.name, &suffix), representation),
            Ok(expected_for(&account, &suffix)));
    }

    #[test]
    fn nonroot_histories_survive_foreign_lookups(history in prop::collection::vec((any::<usize>(), suffix(), 0u8..6, any::<bool>()), 1..30)) {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let choices: Vec<_> = accounts().iter().filter(|a| a.name != "root").collect();
        for (index, suffix, representation, foreign_by_uid) in history {
            let account = choices[index % choices.len()];
            // Simulate unrelated FFI calls between expansions, never retaining
            // or dereferencing a pointer after a subsequent non-reentrant call.
            unsafe {
                if foreign_by_uid { libc::getpwuid(0); }
                else { libc::getpwnam(CString::new("root").unwrap().as_ptr()); }
            }
            prop_assert_eq!(expand_as(&input_for(&account.name, &suffix), representation),
                Ok(expected_for(account, &suffix)));
        }
    }

    #[test]
    fn generated_names_match_presence_oracle(name in "[a-zA-Z_éю日本][a-zA-Z0-9_éю日本-]{0,30}", suffix in suffix(), representation in 0u8..6) {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let expected = match lookup(Some(&name)) {
            Some(account) => Ok(expected_for(&account, &suffix)),
            None => Err(ExpandUserError::UserNotFound { user: name.clone() }),
        };
        prop_assert_eq!(expand_as(&input_for(&name, &suffix), representation), expected);
    }
}

#[test]
#[ignore = "opt-in foreign libc/shared-storage stress; run isolated with --exact --ignored --nocapture"]
fn concurrent_expansions_with_foreign_libc_calls() {
    use std::sync::{Arc, Barrier};
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let accounts: Vec<_> = accounts()
        .iter()
        .filter(|a| a.name != "root")
        .cloned()
        .collect();
    let iterations: usize = std::env::var("EXPANDUSER_STRESS_ITERS")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(20_000);
    let barrier = Arc::new(Barrier::new(6));
    std::thread::scope(|scope| {
        for worker in 0..6 {
            let barrier = Arc::clone(&barrier);
            let accounts = &accounts;
            scope.spawn(move || {
                barrier.wait();
                if worker < 2 {
                    let root = CString::new("root").unwrap();
                    for index in 0..iterations {
                        // Foreign callers own no Rust references to shared libc
                        // memory. Concurrent expansion may still expose the
                        // implementation's unsupported getpwnam use on Linux.
                        unsafe {
                            if (index + worker) % 2 == 0 {
                                libc::getpwnam(root.as_ptr());
                            } else {
                                libc::getpwuid(0);
                            }
                        }
                    }
                } else {
                    for index in 0..iterations {
                        let account = &accounts[(index + worker) % accounts.len()];
                        assert_eq!(
                            format!("~{}/probe", account.name).expand_user(),
                            Ok(account.home.join("probe")),
                            "worker={worker}, iteration={index}, account={account:?}"
                        );
                    }
                }
            });
        }
    });
    eprintln!("stress completed: {} expansions, {} foreign lookups, 6 threads; absence of failure is not proof of safety", iterations * 4, iterations * 2);
}
