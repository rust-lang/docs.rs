use anyhow::{Result, bail};
use docs_rs_uri::EscapedURI;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use strum::VariantArray;

/// alert severity with icon.
/// Used by abnormalities & global alerts
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, VariantArray)]
pub enum AlertSeverity {
    #[default]
    Warn,
    Error,
}

impl fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warn => f.write_str("warn"),
            Self::Error => f.write_str("error"),
        }
    }
}

impl FromStr for AlertSeverity {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("warn") {
            Ok(Self::Warn)
        } else if s.eq_ignore_ascii_case("error") {
            Ok(Self::Error)
        } else {
            bail!("invalid severity: {s}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Abnormality {
    pub url: EscapedURI,
    pub text: String,
    /// explanation to be shown on the status page, can be HTML
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub severity: AlertSeverity,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_is_from_str_for_all_variants() {
        for severity in AlertSeverity::VARIANTS {
            assert_eq!(
                *severity,
                severity.to_string().parse::<AlertSeverity>().unwrap()
            );
        }
    }
}
