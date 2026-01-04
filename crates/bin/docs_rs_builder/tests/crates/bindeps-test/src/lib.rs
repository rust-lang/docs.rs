//! Test crate for bindeps support
//!
//! This crate uses unstable cargo feature `bindeps` (artifact dependencies).
//! It should build on docs.rs when the fix for #2710 is applied.

pub fn hello() -> &'static str {
    "Hello from bindeps-test!"
}

