use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub optional: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Dependencies {
    pub normal: Vec<Dependency>,
    pub development: Vec<Dependency>,
    pub build: Vec<Dependency>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocsrsPackageMetadata {
    /// Crate name.
    pub name: String,

    /// Crate version.
    pub version: String,

    /// Crate description.
    pub description: Option<String>,

    /// Target name.
    ///
    /// This is name of library. Some libraries are using different names in their `[lib]` section.
    /// Crate name (cargo package name) and library name can be different.
    pub target_name: String,

    /// Release time.
    pub release_time: String,

    /// Dependencies defined in Cargo.toml.
    pub dependencies: Dependencies,

    /// Build status of default target.
    pub build_status: bool,

    /// rustdoc status.
    ///
    /// Avability of documentation of a package. Some crates don't have any documentation even
    /// if they are library.
    pub rustdoc_status: bool,

    /// License of crate.
    pub license: Option<String>,

    /// Repository URL.
    pub repository: Option<String>,

    /// Homepage URL.
    pub homepage: Option<String>,

    /// Documentation URL
    pub documentation: Option<String>,

    /// Content of rustdoc of main library.
    pub rustdoc_content: Option<String>,

    /// Content of README.
    pub readme_content: Option<String>,

    /// Authors
    pub authors: Vec<String>,

    /// Keywords
    pub keywords: Vec<String>,

    /// Categories
    pub categories: Vec<String>,

    /// Crate have examples?
    pub have_examples: bool,

    /// Successfully built targets
    pub doc_targets: Vec<String>,

    /// Crate a library?
    pub is_library: bool,

    /// rustc version used to build package
    pub rustc_version: String,

    /// builder version used to build package
    pub builder_version: String,

    /// Default target
    pub default_target: String,
}
