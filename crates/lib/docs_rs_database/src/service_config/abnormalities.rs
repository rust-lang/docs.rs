use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use docs_rs_uri::EscapedURI;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use strum::VariantArray;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorId {
    Manual,
    QueueLength,
}

impl AnchorId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::QueueLength => "queue-length",
        }
    }
}

impl fmt::Display for AnchorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for AnchorId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

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
    pub anchor_id: AnchorId,
    pub url: EscapedURI,
    pub text: String,
    /// explanation to be shown on the status page, can be HTML
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub severity: AlertSeverity,
}

impl Abnormality {
    pub fn topbar_url(&self) -> EscapedURI {
        EscapedURI::from_path("/-/status/").with_fragment(self.anchor_id.as_str())
    }
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
