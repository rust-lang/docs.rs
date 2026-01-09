use anyhow::Result;

/// The main config trait for an application or library config.
///
/// Used across our various binary or library crates.
pub trait AppConfig: Sized {
    fn from_environment() -> Result<Self>;

    #[cfg(feature = "testing")]
    fn test_config() -> Result<Self> {
        Self::from_environment()
    }
}
