// Adapted from RustSec (MIT); see ../LICENSE-MIT.
//! Bounds used when parsing advisory IDs.
pub(super) const YEAR_MIN: u32 = 2000;
pub(super) const YEAR_MAX: u32 = YEAR_MIN + 100;
