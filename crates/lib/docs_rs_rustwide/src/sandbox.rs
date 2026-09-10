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
    /// Number of Cargo jobs matching this CPU restriction, when it is integral.
    pub fn cargo_jobs(&self) -> Option<usize> {
        match self {
            Self::Quota(limit) if limit.fract() == 0.0 && *limit >= 1.0 => Some(*limit as usize),
            Self::Cores(cores) => Some(cores.len()),
            Self::Quota(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildCores(RangeInclusive<usize>);

impl From<RangeInclusive<usize>> for BuildCores {
    fn from(value: RangeInclusive<usize>) -> Self {
        BuildCores(value)
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
    #[error("expected build core range in the form <start>-<end>")]
    MissingSeparator,
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
    #[error("not enough cores, we only have {0}")]
    NotEnoughCores(usize),
}

impl FromStr for BuildCores {
    type Err = ParseBuildCoresError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (start, end) = s
            .split_once('-')
            .ok_or(ParseBuildCoresError::MissingSeparator)?;

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

        if start > end {
            return Err(ParseBuildCoresError::DescendingRange);
        }

        let cpus = num_cpus::get();

        if end >= cpus {
            // NOTE: docker counts the cores zero-based, so
            // a core-number that is exactly the cpu-count is already
            // too high.
            return Err(ParseBuildCoresError::NotEnoughCores(cpus));
        }

        Ok(Self(start..=end))
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
    fn rejects_build_core_range_without_separator() {
        let err = "3".parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains("expected build core range in the form <start>-<end>")
        );
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
    fn rejects_build_core_range_with_invalid_core_number() {
        let cpus = num_cpus::get();
        let err = format!("0-{cpus}").parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains(&format!("not enough cores, we only have {cpus}"))
        );
    }

    // #[test]
    // fn cargo_jobs_uses_core_range_length() {
    //     let config = config_with_cpu_settings(Some(12), Some(BuildCores(3..=4)));

    //     assert_eq!(config.cargo_job_limit(), Some(2));
    // }

    #[test]
    fn derives_cargo_jobs_from_cpu_restrictions() {
        assert_eq!(CpuLimit::Quota(2.0).cargo_jobs(), Some(2));
        assert_eq!(CpuLimit::Quota(0.5).cargo_jobs(), None);
        assert_eq!(CpuLimit::Cores(BuildCores(3..=5)).cargo_jobs(), Some(3));
    }
}
