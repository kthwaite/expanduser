# expanduser

A Rust library for expanding tilde expressions to user home directories.

- Expands `~` to the current user's home directory
- Supports expansion of `~username` to specific user home directories
- Works with both `&str` and `Path` types
- Provides detailed error handling

## Usage

```rust
use expand_user::ExpandUser;

fn main() -> Result<(), expand_user::ExpandUserError> {
    let path = "~/Documents/file.txt";
    let expanded_path = path.expand_user()?;
    println!("Expanded path: {:?}", expanded_path);
    Ok(())
}
```

## API

The library provides a trait, `ExpandUser`, with a single method:

```rust
pub trait ExpandUser {
    fn expand_user(&self) -> Result<PathBuf, ExpandUserError>;
}
```

This trait is implemented for any type that implements `AsRef<Path>`, including `&str` and `Path`.


### Error Handling

The library defines an `ExpandUserError` enum to handle various error cases:

- `CurrentUserHomeNotFound`: The current user's home directory couldn't be found
- `UserNotFound`: The specified user doesn't exist
- `UserHomeNotFound`: The home directory for the specified user couldn't be found
- `InvalidTildeExpression`: The provided tilde expression is invalid
