mod build;
mod errored_build;
mod github_stats;
mod legacy;

pub use build::FakeBuild;
pub use docs_rs_registry_api::{CrateOwner, OwnerKind};
pub use github_stats::FakeGithubStats;
pub use legacy::{FakeRelease, fake_release_that_failed_before_build};
