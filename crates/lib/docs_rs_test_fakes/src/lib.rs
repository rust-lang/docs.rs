mod legacy;

pub use docs_rs_registry_api::{CrateOwner, OwnerKind};
pub use legacy::{FakeBuild, FakeRelease, fake_release_that_failed_before_build};
