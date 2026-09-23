mod mock;

pub use mock::MockStdReplacements;

use crate::ReplacementDetails;

/// Create replacement details with the given description and a fixed example URL.
pub fn std_replacement(description: &str) -> ReplacementDetails {
    ReplacementDetails::new(
        description,
        "https://example.com/replacement".parse().unwrap(),
    )
}
