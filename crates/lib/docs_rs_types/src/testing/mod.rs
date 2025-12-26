use crate::{KrateName, Version};

// testing krate name constants
pub const KRATE: KrateName = KrateName::from_static("krate");
pub const FOO: KrateName = KrateName::from_static("foo");
pub const BAR: KrateName = KrateName::from_static("bar");
pub const BAZ: KrateName = KrateName::from_static("baz");
pub const OTHER: KrateName = KrateName::from_static("other");

// some versions as constants for tests
pub const V0_1: Version = Version::new(0, 1, 0);
pub const V1: Version = Version::new(1, 0, 0);
pub const V2: Version = Version::new(2, 0, 0);
pub const V3: Version = Version::new(3, 0, 0);

pub const DEFAULT_TARGET: &str = "x86_64-unknown-linux-gnu";
