//! A wrapper around using rustup
//!

use std::process::Command;

/// Invoke rustup in a folder to `override set` a rustc version
pub fn set_version(v: String) {
    Command::new("rustup")
        .arg("override")
        .arg("set")
        .arg(v)
        .output()
        .unwrap();
}
