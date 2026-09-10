use std::{
    num::ParseIntError,
    ops::{Deref, RangeInclusive},
    str::FromStr,
};
use thiserror::Error;

/// CPU restriction applied to the build sandbox.
#[derive(Clone, Debug, PartialEq)]
pub enum CpuLimit {
    /// Restrict the container to a fraction or number of CPU cores.
    Quota(f32),
    /// Pin the container to an inclusive range of host CPU IDs.
    Cores(BuildCores),
}

impl CpuLimit {
    /// Reject invalid quotas before initializing the workspace or starting Docker.
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Self::Quota(quota) = self {
            anyhow::ensure!(
                quota.is_finite() && *quota > 0.0,
                "CPU quota must be a positive finite number"
            );
        }
        Ok(())
    }

    /// Number of Cargo jobs matching this CPU restriction, when it is integral.
    pub fn cargo_jobs(&self) -> Option<usize> {
        match self {
            Self::Quota(limit) if limit.fract() == 0.0 && *limit >= 1.0 => Some(*limit as usize),
            Self::Cores(cores) => Some(cores.len()),
            Self::Quota(_) => None,
        }
    }
}

/// A nonempty inclusive CPU-ID range, parsed as `CORE` or `START-END`.
/// Docker checks CPU availability on the daemon host; parsing is host-independent.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildCores(RangeInclusive<usize>);

impl TryFrom<RangeInclusive<usize>> for BuildCores {
    type Error = ParseBuildCoresError;
    fn try_from(value: RangeInclusive<usize>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(ParseBuildCoresError::DescendingRange);
        }
        if (value.end() - value.start()).checked_add(1).is_none() {
            return Err(ParseBuildCoresError::RangeTooLarge);
        }
        Ok(Self(value))
    }
}

impl From<BuildCores> for RangeInclusive<usize> {
    fn from(value: BuildCores) -> Self {
        value.0
    }
}

impl From<&BuildCores> for RangeInclusive<usize> {
    fn from(value: &BuildCores) -> Self {
        value.clone().0
    }
}

impl BuildCores {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.size_hint().0
    }
}

impl Deref for BuildCores {
    type Target = RangeInclusive<usize>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Error)]
pub enum ParseBuildCoresError {
    #[error("invalid build core range start `{value}`: {source}")]
    InvalidStart {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("invalid build core range end `{value}`: {source}")]
    InvalidEnd {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("build core range start must be less than or equal to end")]
    DescendingRange,
    #[error("CPU core range length does not fit usize")]
    RangeTooLarge,
}

impl FromStr for BuildCores {
    type Err = ParseBuildCoresError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (start, end) = s.split_once('-').unwrap_or((s, s));

        let start = start
            .parse()
            .map_err(|source| ParseBuildCoresError::InvalidStart {
                value: start.to_string(),
                source,
            })?;

        let end = end
            .parse()
            .map_err(|source| ParseBuildCoresError::InvalidEnd {
                value: end.to_string(),
                source,
            })?;

        Self::try_from(start..=end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_core_range() {
        let build_cores: BuildCores = "2-3".parse().unwrap();

        assert_eq!(build_cores.start(), &2);
        assert_eq!(build_cores.end(), &3);
        assert_eq!(build_cores.len(), 2);
    }

    #[test]
    fn parses_single_build_core() {
        let build_cores: BuildCores = "2-2".parse().unwrap();

        assert_eq!(build_cores.start(), &2);
        assert_eq!(build_cores.end(), &2);
        assert_eq!(build_cores.len(), 1);
    }

    #[test]
    fn parses_single_core_without_separator() {
        assert_eq!("3".parse::<BuildCores>().unwrap(), BuildCores(3..=3));
    }

    #[test]
    fn rejects_build_core_range_with_descending_values() {
        let err = "4-3".parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains("build core range start must be less than or equal to end")
        );
    }

    #[test]
    fn rejects_build_core_range_with_invalid_end() {
        let err = "3-a".parse::<BuildCores>().unwrap_err();

        assert!(err.to_string().contains("invalid build core range end `a`"));
    }

    #[test]
    fn validates_ranges_without_relying_on_host_cpu_count() {
        assert_eq!("1024-1025".parse::<BuildCores>().unwrap().len(), 2);
        assert!(BuildCores::try_from(RangeInclusive::new(4, 3)).is_err());
        assert!(BuildCores::try_from(0..=usize::MAX).is_err());
    }

    #[test]
    fn rejects_invalid_quotas() {
        for quota in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(CpuLimit::Quota(quota).validate().is_err());
        }
        assert!(CpuLimit::Quota(0.5).validate().is_ok());
        assert!(CpuLimit::Quota(2.0).validate().is_ok());
    }

    #[test]
    fn derives_cargo_jobs_from_cpu_restrictions() {
        assert_eq!(CpuLimit::Quota(2.0).cargo_jobs(), Some(2));
        assert_eq!(CpuLimit::Quota(0.5).cargo_jobs(), None);
        assert_eq!(CpuLimit::Cores(BuildCores(3..=5)).cargo_jobs(), Some(3));
    }
}
