//! The token's place under `XDG_CONFIG_HOME`. It's alone in its own test
//! binary because it sets environment variables, which isn't safe while
//! other tests' threads may be reading them.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use taskboard_web::auth::{load_or_create_token, token_path};

#[test]
fn the_token_is_made_under_xdg_config_home_and_reused() {
    let config = tempfile::tempdir().unwrap();
    // SAFETY: this binary's only test, so no other thread reads the
    // environment meanwhile.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };

    let path = token_path().unwrap();
    assert_eq!(path, config.path().join("tb/web-token"));
    let token = load_or_create_token(&path).unwrap();
    assert!(token.len() >= 32, "{token}");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token);
    assert_eq!(load_or_create_token(&token_path().unwrap()).unwrap(), token);
}
