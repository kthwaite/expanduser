use expanduser::ExpandUser;
use std::{fs, path::Path};

// Keep HOME mutation in this single-test integration binary, separate from the
// library's environment-dependent unit tests.
#[test]
fn expansion_preserves_file_and_directory_requirements() {
    let plain = tempfile::NamedTempFile::new_in(".").unwrap();
    let name = Path::new(plain.path().file_name().unwrap());
    assert!(fs::metadata(name).unwrap().is_file());
    assert!(fs::metadata(name.expand_user().unwrap()).unwrap().is_file());

    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("file");
    fs::write(&file, b"contents").unwrap();
    std::env::set_var("HOME", home.path());
    for ending in ["/", "/."] {
        let literal = format!("{}{ending}", file.display());
        let tilde = format!("~/file{ending}");
        for input in [&literal, &tilde] {
            let expanded = input.expand_user().unwrap();
            assert_eq!(
                fs::metadata(expanded).unwrap_err().raw_os_error(),
                Some(libc::ENOTDIR),
                "directory requirement lost for {input:?}"
            );
        }
    }

    // Expansion is lexical: it must not add a directory requirement even if
    // HOME points at a file rather than an existing directory.
    std::env::set_var("HOME", &file);
    let expanded = "~".expand_user().unwrap();
    assert_eq!(fs::read(expanded).unwrap(), b"contents");
}
