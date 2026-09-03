use std::ops::RangeInclusive;

/// CPU restriction applied to the build sandbox.
#[derive(Clone, Debug, PartialEq)]
pub enum CpuLimit {
    /// Restrict the container to a fraction or number of CPU cores.
    Quota(f32),
    /// Pin the container to an inclusive range of host CPU IDs.
    Cores(RangeInclusive<usize>),
}

impl CpuLimit {
    /// Number of Cargo jobs matching this CPU restriction, when it is integral.
    pub fn cargo_jobs(&self) -> Option<usize> {
        match self {
            Self::Quota(limit) if limit.fract() == 0.0 && *limit >= 1.0 => Some(*limit as usize),
            Self::Cores(cores) => Some(cores.clone().count()),
            Self::Quota(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_cargo_jobs_from_cpu_restrictions() {
        assert_eq!(CpuLimit::Quota(2.0).cargo_jobs(), Some(2));
        assert_eq!(CpuLimit::Quota(0.5).cargo_jobs(), None);
        assert_eq!(CpuLimit::Cores(3..=5).cargo_jobs(), Some(3));
    }
}
