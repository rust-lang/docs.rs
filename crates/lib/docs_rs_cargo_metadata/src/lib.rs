mod metadata;
mod release_dependency;

pub use metadata::{CargoMetadata, Dependency, Package as MetadataPackage, Target};
pub use release_dependency::{ReleaseDependency, ReleaseDependencyList};
