use std::{
    num::ParseIntError,
    ops::{Deref, RangeInclusive},
    str::FromStr,
};
use thiserror::Error;

/// A positive, finite number of CPUs available to the sandbox.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuQuota(f32);

/// A CPU quota must be a positive, finite number.
#[derive(Clone, Copy, Debug, Error)]
#[error("CPU quota must be a positive finite number")]
pub struct InvalidCpuQuota;

impl TryFrom<f32> for CpuQuota {
    type Error = InvalidCpuQuota;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(InvalidCpuQuota)
        }
    }
}

impl FromStr for CpuQuota {
    type Err = InvalidCpuQuota;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.parse::<f32>().map_err(|_| InvalidCpuQuota)?)
    }
}

impl CpuQuota {
    /// The validated CPU quota passed to Docker.
    pub fn get(self) -> f32 {
        self.0
    }
}

/// CPU restriction applied to the build sandbox.
#[derive(Clone, Debug, PartialEq)]
pub enum CpuLimit {
    /// Restrict the container to a fraction or number of CPU cores.
    Quota(CpuQuota),
    /// Pin the container to an inclusive range of host CPU IDs.
    Cores(BuildCores),
}

impl CpuLimit {
    /// Number of Cargo jobs matching this CPU restriction, when it is integral.
    pub fn cargo_jobs(&self) -> Option<usize> {
        match self {
            Self::Quota(limit) if limit.get().fract() == 0.0 => Some(limit.get() as usize),
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

impl BuildCores {
    pub fn get(&self) -> RangeInclusive<usize> {
        self.0.clone()
    }

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
            assert!(CpuQuota::try_from(quota).is_err());
        }
        assert_eq!(CpuQuota::try_from(0.5).unwrap().get(), 0.5);
        assert_eq!(CpuQuota::try_from(2.0).unwrap().get(), 2.0);
    }

    #[test]
    fn parses_only_valid_quotas() {
        for value in ["", "no", "0", "-0", "-1", "NaN", "inf", "-inf", "1e100"] {
            assert!(value.parse::<CpuQuota>().is_err(), "{value}");
        }
        assert_eq!("0.5".parse::<CpuQuota>().unwrap().get(), 0.5);
        assert_eq!("2".parse::<CpuQuota>().unwrap().get(), 2.0);
    }

    #[test]
    fn derives_cargo_jobs_from_cpu_restrictions() {
        assert_eq!(
            CpuLimit::Quota(2.0.try_into().unwrap()).cargo_jobs(),
            Some(2)
        );
        assert_eq!(CpuLimit::Quota(0.5.try_into().unwrap()).cargo_jobs(), None);
        assert_eq!(CpuLimit::Cores(BuildCores(3..=5)).cargo_jobs(), Some(3));
    }
}
