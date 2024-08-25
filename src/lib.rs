//! A library for expanding tilde expressions to a user's home directories.
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

/// An error that can occur when expanding a tilde expression.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ExpandUserError {
    #[error("Current user's $HOME directory not found")]
    CurrentUserHomeNotFound,
    #[error("User {user} not found")]
    UserNotFound { user: String },
    #[error("$HOME directory for {user} not found")]
    UserHomeNotFound { user: String },
    #[error("Failed to expand tilde expression: {expr}")]
    InvalidTildeExpression { expr: String },
}

impl ExpandUserError {
    /// Creates an error for when the current user's home directory could not be found.
    pub fn user_not_found<S: AsRef<str>>(user: S) -> Self {
        Self::UserNotFound {
            user: user.as_ref().to_owned(),
        }
    }
}

/// Returns the home directory for a user identified by name.
/// # Safety
/// This function is marked unsafe because it calls the unsafe `getpwnam` and
/// `Cstr::from_ptr` functions, and dereferences the raw pointer returned by `getpwnam`.
unsafe fn get_user_home(user: &str) -> Result<OsString, ExpandUserError> {
    let user_cstr = match CString::new(user) {
        Ok(user) => user,
        Err(_) => {
            return Err(ExpandUserError::InvalidTildeExpression {
                expr: user.to_string(),
            });
        }
    };
    let ptr = libc::getpwnam(user_cstr.as_ptr());
    if ptr.is_null() {
        return Err(ExpandUserError::UserNotFound {
            user: user.to_string(),
        });
    }
    let pw_dir = (*ptr).pw_dir;
    if pw_dir.is_null() {
        return Err(ExpandUserError::UserHomeNotFound {
            user: user.to_string(),
        });
    }

    let pw_dir = CStr::from_ptr(pw_dir).to_bytes();
    Ok(OsStr::from_bytes(pw_dir).to_owned())
}

pub trait ExpandUser {
    fn expand_user(&self) -> Result<PathBuf, ExpandUserError>;
}

impl<T> ExpandUser for T
where
    T: AsRef<Path>,
{
    /// Expands a tilde expression to a user's home directory.
    fn expand_user(&self) -> Result<PathBuf, ExpandUserError> {
        let mut components = self.as_ref().components();
        let mut path = PathBuf::new();
        match components.next() {
            Some(Component::Normal(part)) => match part.to_str() {
                Some("~") => match dirs_sys::home_dir() {
                    Some(dir) => path.push(dir),
                    None => return Err(ExpandUserError::CurrentUserHomeNotFound),
                },
                Some(prefix) if prefix.starts_with("~") => {
                    let assumed_user = &prefix[1..];
                    if assumed_user == "root" {
                        path.push("/root");
                    } else {
                        let home_dir = unsafe { get_user_home(assumed_user)? };
                        path.push(home_dir);
                    }
                }
                _ => path.push(part),
            },
            Some(other) => path.push(other),
            None => return Ok(path),
        }
        path.push(components);
        Ok(path)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::env;
    use std::fs;
    use std::os::unix::fs as unix_fs;
    use std::os::unix::fs::PermissionsExt;

    macro_rules! setenv_home {
        ($path: expr) => {
            std::env::set_var("HOME", Path::new($path).as_os_str());
        };
    }

    macro_rules! expand_user_eq {
        ($path: expr, $cmp: expr) => {
            assert_eq!($path.expand_user().unwrap(), PathBuf::from($cmp),);
        };
    }

    #[test]
    fn test_expand_user_from_str() {
        expand_user_eq!("", "");
        expand_user_eq!("/", "/");

        setenv_home!("/Users/kinbote");
        expand_user_eq!("~", "/Users/kinbote");
        expand_user_eq!("~/", "/Users/kinbote");
        expand_user_eq!(
            "~/.config/prog/config.json",
            "/Users/kinbote/.config/prog/config.json"
        );
    }

    #[test]
    fn test_expand_user_from_path() {
        expand_user_eq!(Path::new(""), "");
        expand_user_eq!(Path::new("/"), "/");

        setenv_home!("/Users/kinbote");
        expand_user_eq!(Path::new("~"), "/Users/kinbote");
        expand_user_eq!(Path::new("~/"), "/Users/kinbote");
        expand_user_eq!(
            Path::new("~/.config/prog/config.json"),
            "/Users/kinbote/.config/prog/config.json"
        );
    }

    #[test]
    fn test_root_expansion() {
        expand_user_eq!("~root", "/root");
        expand_user_eq!("~root/.bashrc", "/root/.bashrc");
    }

    #[test]
    fn test_error_cases() {
        assert!(Path::new("~nonexistentuser").expand_user().is_err());

        assert!(Path::new("~:invalid").expand_user().is_err());
    }

    #[test]
    fn test_edge_cases() {
        setenv_home!("/Users/kinbote");

        // Multiple tilde expressions
        expand_user_eq!("~/foo/~root/bar", "/Users/kinbote/foo/~root/bar");

        // Tilde not at start
        expand_user_eq!("/foo/~/bar", "/foo/~/bar");

        // Empty string as username
        expand_user_eq!("~", "/Users/kinbote");
        expand_user_eq!("~/", "/Users/kinbote");
    }

    #[test]
    fn test_different_path_types() {
        setenv_home!("/Users/kinbote");

        // PathBuf
        let path_buf = PathBuf::from("~/documents");
        expand_user_eq!(path_buf, "/Users/kinbote/documents");

        // OsString
        let os_string = OsString::from("~/documents");
        expand_user_eq!(os_string, "/Users/kinbote/documents");
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_handling() {
        setenv_home!("/Users/kinbote");

        // Create a temporary directory and symlink
        let temp_dir = tempfile::tempdir().unwrap();
        let symlink_path = temp_dir.path().join("symlink");
        unix_fs::symlink("/Users/kinbote/target", &symlink_path).unwrap();

        expand_user_eq!(symlink_path, symlink_path);
    }

    #[test]
    fn test_unicode_handling() {
        setenv_home!("/Users/kinbote");

        expand_user_eq!("~/документы", "/Users/kinbote/документы");
        // Note: This assumes the existence of a user with a Unicode name, which might not be common
        // expand_user_eq!("~юзер", "/home/юзер");
    }

    #[test]
    fn test_long_paths() {
        setenv_home!("/Users/kinbote");

        let long_suffix = "a".repeat(1000);
        let long_path = format!("~/{}", long_suffix);
        let expected = format!("/Users/kinbote/{}", long_suffix);
        expand_user_eq!(long_path, expected);
    }

    #[test]
    fn test_relative_and_absolute_paths() {
        setenv_home!("/Users/kinbote");

        // Relative path
        expand_user_eq!("~/documents", "/Users/kinbote/documents");

        // Absolute path
        expand_user_eq!("/absolute/path", "/absolute/path");
    }

    #[test]
    #[cfg(unix)]
    fn test_permission_scenarios() {
        // Create a temporary directory with restricted permissions
        let temp_dir = tempfile::tempdir().unwrap();
        let restricted_dir = temp_dir.path().join("restricted");
        fs::create_dir(&restricted_dir).unwrap();
        fs::set_permissions(&restricted_dir, fs::Permissions::from_mode(0o000)).unwrap();

        // This should work, even though we can't access the directory
        expand_user_eq!(restricted_dir.join("file"), restricted_dir.join("file"));

        // Clean up
        fs::set_permissions(&restricted_dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
