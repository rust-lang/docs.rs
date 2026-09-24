#![warn(missing_docs)]

//! Parse docs.rs build metadata from a crate's Cargo.toml.
//!
//! This library exposes feature selections, compiler arguments, environment
//! variables, and documentation targets. Command construction and execution live
//! in `docs_rs_rustwide`.
//! See <https://docs.rs/about/metadata> for supported metadata settings.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use docsrs_metadata::Metadata;
//!
//! let metadata = Metadata::from_crate_root(env!("CARGO_MANIFEST_DIR"))?;
//! let targets = metadata.targets(/* include_default_targets: */ true);
//! println!("default documentation target: {}", targets.default_target);
//! println!("additional rustdoc arguments: {:?}", metadata.rustdoc_args);
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;
use toml::Value;

/// The target that `metadata` is being built for.
///
/// This is directly passed on from the Cargo [`TARGET`] variable.
///
/// [`TARGET`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts
pub const HOST_TARGET: &str = env!("DOCSRS_METADATA_HOST_TARGET");
/// The targets that are built if no `targets` section is specified.
///
/// Currently, this is guaranteed to have only [tier one] targets.
/// However, it may not contain all tier one targets.
///
/// [tier one]: https://doc.rust-lang.org/nightly/rustc/platform-support.html#tier-1
pub const DEFAULT_TARGETS: &[&str] = &[
    "i686-pc-windows-msvc",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

/// The possible errors for [`Metadata::from_crate_root`].
#[derive(Debug, Error)]
#[allow(clippy::upper_case_acronyms)]
#[non_exhaustive]
pub enum MetadataError {
    /// The error returned when the manifest could not be read.
    #[error("failed to read manifest from disk")]
    IO(#[from] io::Error),
    /// The error returned when the manifest could not be parsed.
    #[error("failed to parse manifest")]
    Parse(#[from] toml::de::Error),
}

/// Metadata to set for custom builds.
///
/// This metadata is read from `[package.metadata.docs.rs]` table in `Cargo.toml`.
///
/// An example metadata:
///
/// ```text
/// [package]
/// name = "test"
///
/// [package.metadata.docs.rs]
/// features = [ "feature1", "feature2" ]
/// all-features = true
/// no-default-features = true
/// default-target = "x86_64-unknown-linux-gnu"
/// targets = [ "aarch64-apple-darwin", "x86_64-pc-windows-msvc" ]
/// additional-targets = [ "i686-apple-darwin" ]
/// rustc-args = [ "--example-rustc-arg" ]
/// rustdoc-args = [ "--example-rustdoc-arg" ]
/// ```
///
/// You can define one or more fields in your `Cargo.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Metadata {
    /// Whether the current crate is a proc-macro (used by docs.rs to hack around cargo bugs).
    #[serde(default)]
    pub proc_macro: bool,

    /// List of features to pass on to `cargo`.
    ///
    /// By default, docs.rs will only build default features.
    pub features: Option<Vec<String>>,

    /// Whether to pass `--all-features` to `cargo`.
    #[serde(default)]
    pub all_features: bool,

    /// Whether to pass `--no-default-features` to `cargo`.
    //
    /// By default, Docs.rs will build default features.
    /// Set `no-default-features` to `true` if you want to build only certain features.
    #[serde(default)]
    pub no_default_features: bool,

    /// See [`BuildTargets`].
    default_target: Option<String>,
    targets: Option<Vec<String>>,

    /// List of command line arguments for `rustc`.
    #[serde(default)]
    pub rustc_args: Vec<String>,

    /// List of command line arguments for `rustdoc`.
    #[serde(default)]
    pub rustdoc_args: Vec<String>,

    /// List of command line arguments for `cargo`.
    ///
    /// These cannot be a subcommand, they may only be options.
    #[serde(default)]
    pub cargo_args: Vec<String>,

    /// List of additional targets to be generated. See [`BuildTargets`].
    #[serde(default)]
    additional_targets: Vec<String>,
}

/// The targets that should be built for a crate.
///
/// The `default_target` is the target to be used as the home page for that crate.
///
/// # See also
/// - [`Metadata::targets`]
pub struct BuildTargets<'a> {
    /// The target that should be built by default.
    ///
    /// If `default_target` is unset and `targets` is non-empty,
    /// the first element of `targets` will be used as the `default_target`.
    /// Otherwise, this defaults to [`HOST_TARGET`].
    pub default_target: &'a str,

    /// Which targets should be built.
    ///
    /// If you want a crate to build only for specific targets,
    /// set `targets` to the list of targets to build, in addition to `default-target`.
    ///
    /// If `targets` is not set, all [`DEFAULT_TARGETS`] will be built.
    /// If `targets` is set to an empty array, only the default target will be built.
    /// If `targets` is set to a non-empty array but `default_target` is not set,
    ///    the first element will be treated as the default.
    pub other_targets: HashSet<&'a str>,
}

impl Metadata {
    /// Read the `Cargo.toml` from a source directory, then parse the build metadata.
    ///
    /// If you already have the path to a TOML file, use [`Metadata::from_manifest`] instead.
    pub fn from_crate_root<P: AsRef<Path>>(source_dir: P) -> Result<Metadata, MetadataError> {
        let manifest_path = source_dir.as_ref().join("Cargo.toml");
        if manifest_path.exists() {
            Metadata::from_manifest(manifest_path)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "no Cargo.toml").into())
        }
    }

    /// Read the given file into a string, then parse the build metadata.
    ///
    /// If you already have the TOML as a string, use [`from_str`] instead.
    /// If you just want the default settings, use [`Metadata::default()`][Default::default].
    ///
    /// [`from_str`]: std::str::FromStr
    pub fn from_manifest<P: AsRef<Path>>(path: P) -> Result<Metadata, MetadataError> {
        use std::{fs, str::FromStr};
        let buf = fs::read_to_string(path)?;
        Metadata::from_str(&buf).map_err(Into::into)
    }

    /// Return the targets that should be built.
    ///
    /// The `default_target` will never be one of the `other_targets`.
    /// If `include_default_targets` is `true` and `targets` is unset, this also includes
    /// [`DEFAULT_TARGETS`]. Otherwise, if `include_default_targets` is `false` and `targets`
    /// is unset, `other_targets` will be empty.
    ///
    /// All of the above is ignored for proc-macros, which are always only compiled for the host.
    pub fn targets(&self, include_default_targets: bool) -> BuildTargets<'_> {
        self.targets_for_host(include_default_targets, HOST_TARGET)
    }

    /// Return the targets that should be built, given a different simulated HOST_TARGET.
    pub fn targets_for_host(
        &self,
        include_default_targets: bool,
        host_target: &'static str,
    ) -> BuildTargets<'_> {
        // Proc macros can only be compiled for the host, so just completely ignore any configured targets.
        // It would be nice to warn about this somehow ...
        if self.proc_macro {
            return BuildTargets {
                default_target: host_target,
                other_targets: HashSet::default(),
            };
        }

        let default_target = self
            .default_target
            .as_deref()
            // Use the first element of `targets` if `default_target` is unset and `targets` is non-empty
            .or_else(|| {
                self.targets
                    .as_ref()
                    .and_then(|targets| targets.first().map(String::as_str))
            })
            .unwrap_or(host_target);

        let crate_targets = self
            .targets
            .as_ref()
            .map(|targets| targets.iter().map(String::as_str).collect());
        // Let people opt-in to only having specific targets
        let mut targets: HashSet<_> = if include_default_targets {
            crate_targets.unwrap_or_else(|| DEFAULT_TARGETS.iter().copied().collect())
        } else {
            crate_targets.unwrap_or_default()
        };
        for additional_target in &self.additional_targets {
            targets.insert(additional_target);
        }

        targets.remove(&default_target);
        BuildTargets {
            default_target,
            other_targets: targets,
        }
    }

    /// Return the environment variables that should be set when building this crate.
    pub fn environment_variables(&self) -> HashMap<&'static str, String> {
        let mut map = HashMap::new();
        // For docs.rs detection from build scripts:
        // https://github.com/rust-lang/docs.rs/issues/147
        map.insert("DOCS_RS", "1".into());
        map
    }
}

impl std::str::FromStr for Metadata {
    type Err = toml::de::Error;

    /// Parse the given manifest as TOML.
    fn from_str(manifest: &str) -> Result<Metadata, Self::Err> {
        use toml::value::Table;

        // the `Cargo.toml` is a full document, and:
        // > A TOML document is represented with the Table type which maps
        // > String to the Value enum.
        let manifest = manifest.parse::<Table>()?;

        fn table<'a>(manifest: &'a Table, table_name: &str) -> Option<&'a Table> {
            match manifest.get(table_name) {
                Some(Value::Table(table)) => Some(table),
                _ => None,
            }
        }

        let package_metadata = table(&manifest, "package").and_then(|t| table(t, "metadata"));

        let plain_table = package_metadata
            .and_then(|t| table(t, "docs"))
            .and_then(|t| table(t, "rs"));

        let quoted_table = package_metadata.and_then(|t| table(t, "docs.rs"));

        let mut metadata = if let Some(table) = plain_table {
            Value::Table(table.clone()).try_into()?
        } else if let Some(table) = quoted_table {
            Value::Table(table.clone()).try_into()?
        } else {
            Metadata::default()
        };

        let proc_macro = table(&manifest, "lib")
            .and_then(|table| table.get("proc-macro").or_else(|| table.get("proc_macro")))
            .and_then(|val| val.as_bool());
        if let Some(proc_macro) = proc_macro {
            metadata.proc_macro = proc_macro;
        }

        metadata.rustdoc_args.push("-Z".into());
        metadata.rustdoc_args.push("unstable-options".into());

        Ok(metadata)
    }
}

#[cfg(test)]
mod test_parsing {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_docsrs_metadata() {
        let manifest = r#"
            [package]
            name = "test"

            [package.metadata.docs.rs]
            features = [ "feature1", "feature2" ]
            all-features = true
            no-default-features = true
            default-target = "x86_64-unknown-linux-gnu"
            targets = [ "aarch64-apple-darwin", "x86_64-pc-windows-msvc" ]
            rustc-args = [ "--example-rustc-arg" ]
            rustdoc-args = [ "--example-rustdoc-arg" ]
            cargo-args = [ "-Zbuild-std" ]
        "#;

        let metadata = Metadata::from_str(manifest).unwrap();

        assert!(metadata.features.is_some());
        assert!(metadata.all_features);
        assert!(metadata.no_default_features);
        assert!(metadata.default_target.is_some());
        assert!(!metadata.proc_macro);

        let features = metadata.features.unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[0], "feature1".to_owned());
        assert_eq!(features[1], "feature2".to_owned());

        assert_eq!(
            metadata.default_target.unwrap(),
            "x86_64-unknown-linux-gnu".to_owned()
        );

        let targets = metadata.targets.expect("should have explicit target");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0], "aarch64-apple-darwin");
        assert_eq!(targets[1], "x86_64-pc-windows-msvc");

        let rustc_args = metadata.rustc_args;
        assert_eq!(rustc_args.len(), 1);
        assert_eq!(rustc_args[0], "--example-rustc-arg".to_owned());

        let rustdoc_args = metadata.rustdoc_args;
        assert_eq!(rustdoc_args.len(), 3);
        assert_eq!(rustdoc_args[0], "--example-rustdoc-arg".to_owned());
        assert_eq!(rustdoc_args[1], "-Z".to_owned());
        assert_eq!(rustdoc_args[2], "unstable-options".to_owned());

        let cargo_args = metadata.cargo_args;
        assert_eq!(cargo_args.as_slice(), &["-Zbuild-std"]);
    }

    #[test]
    fn test_no_targets() {
        // metadata section but no targets
        let manifest = r#"
            [package]
            name = "test"

            [package.metadata.docs.rs]
            features = [ "feature1", "feature2" ]
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();
        assert!(metadata.targets.is_none());

        // no package.metadata.docs.rs section
        let metadata = Metadata::from_str(
            r#"
            [package]
            name = "test"
        "#,
        )
        .unwrap();
        assert!(metadata.targets.is_none());

        // targets explicitly set to empty array
        let metadata = Metadata::from_str(
            r#"
            [package.metadata.docs.rs]
            targets = []
        "#,
        )
        .unwrap();
        assert!(metadata.targets.unwrap().is_empty());
    }

    #[test]
    fn test_quoted_table() {
        // parse quoted keys
        let manifest = r#"
            [package]
            name = "test"
            [package.metadata."docs.rs"]
            features = [ "feature1", "feature2" ]
            all-features = true
            no-default-features = true
            default-target = "x86_64-unknown-linux-gnu"
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();

        assert!(metadata.features.is_some());
        assert!(metadata.all_features);
        assert!(metadata.no_default_features);
        assert!(metadata.default_target.is_some());
    }

    #[test]
    fn test_proc_macro() {
        let manifest = r#"
            [package]
            name = "x"
            [lib]
            proc-macro = true
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();
        assert!(metadata.proc_macro);

        let manifest = r#"
            [package]
            name = "x"
            [lib]
            proc_macro = true
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();
        assert!(metadata.proc_macro);

        // Cargo prioritizes `proc-macro` over `proc_macro` in local testing
        let manifest = r#"
            [package]
            name = "x"
            [lib]
            proc_macro = false
            proc-macro = true
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();
        assert!(metadata.proc_macro);

        let manifest = r#"
            [package]
            name = "x"
            [lib]
            proc-macro = false
            proc_macro = true
        "#;
        let metadata = Metadata::from_str(manifest).unwrap();
        assert!(!metadata.proc_macro);
    }
}

#[cfg(test)]
mod test_targets {
    use super::*;

    #[test]
    fn test_select_targets() {
        let mut metadata = Metadata::default();

        // unchanged default_target, targets not specified
        let BuildTargets {
            default_target: default,
            other_targets: tier_one,
        } = metadata.targets(true);
        assert_eq!(default, HOST_TARGET);

        // should be equal to TARGETS \ {HOST_TARGET}
        for actual in &tier_one {
            assert!(DEFAULT_TARGETS.contains(actual));
        }

        for expected in DEFAULT_TARGETS {
            if *expected == HOST_TARGET {
                assert!(!tier_one.contains(&HOST_TARGET));
            } else {
                assert!(tier_one.contains(expected));
            }
        }

        // unchanged default_target, targets specified to be empty
        metadata.targets = Some(Vec::new());

        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, HOST_TARGET);
        assert!(others.is_empty());

        // unchanged default_target, targets non-empty
        metadata.targets = Some(vec![
            "i686-pc-windows-msvc".into(),
            "i686-apple-darwin".into(),
        ]);

        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, "i686-pc-windows-msvc");
        assert_eq!(others.len(), 1);
        assert!(others.contains(&"i686-apple-darwin"));

        // make sure that default_target is not built twice
        metadata.targets = Some(vec![HOST_TARGET.into()]);
        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, HOST_TARGET);
        assert!(others.is_empty());

        // make sure that duplicates are removed
        metadata.targets = Some(vec![
            "i686-pc-windows-msvc".into(),
            "i686-pc-windows-msvc".into(),
        ]);

        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, "i686-pc-windows-msvc");
        assert!(others.is_empty());

        // make sure that `default_target` always takes priority over `targets`
        metadata.default_target = Some("i686-apple-darwin".into());
        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, "i686-apple-darwin");
        assert_eq!(others.len(), 1);
        assert!(others.contains(&"i686-pc-windows-msvc"));

        // make sure that `default_target` takes priority over `HOST_TARGET`
        metadata.targets = Some(vec![]);
        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, "i686-apple-darwin");
        assert!(others.is_empty());

        // and if `targets` is unset, it should still be set to `TARGETS`
        metadata.targets = None;
        let BuildTargets {
            default_target: default,
            other_targets: others,
        } = metadata.targets(true);

        assert_eq!(default, "i686-apple-darwin");
        let tier_one_targets_no_default = DEFAULT_TARGETS
            .iter()
            .filter(|&&t| t != "i686-apple-darwin")
            .copied()
            .collect();

        assert_eq!(others, tier_one_targets_no_default);
    }

    #[test]
    fn test_additional_targets() {
        let mut metadata = Metadata {
            targets: Some(
                DEFAULT_TARGETS
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            ),
            ..Default::default()
        };

        let additional_target = "i686-apple-darwin";
        metadata.additional_targets = vec![additional_target.to_string()];
        let BuildTargets {
            other_targets: others,
            ..
        } = metadata.targets(true);

        assert!(others.contains(additional_target), "no additional target");
        for target in DEFAULT_TARGETS.iter().skip(1) {
            assert!(others.contains(target), "missing {target}");
        }

        // Now we check that `additional_targets` also works if `targets` is set.
        let target = "i686-pc-windows-msvc";
        metadata.targets = Some(vec![target.to_string()]);
        let BuildTargets {
            other_targets: others,
            default_target: default,
        } = metadata.targets(true);
        assert_eq!(others.len(), 1);
        assert!(others.contains(additional_target));
        assert_eq!(default, target);
    }

    #[test]
    fn no_default_targets() {
        // if `targets` is unset, `other_targets` should be empty
        let metadata = Metadata::default();
        let BuildTargets {
            other_targets: others,
            ..
        } = metadata.targets(false);
        assert!(others.is_empty(), "{others:?}");
    }
}

#[cfg(test)]
mod test_environment {
    use super::*;

    #[test]
    fn default_environment() {
        let env = Metadata::default().environment_variables();
        assert_eq!(env.get("DOCS_RS").map(String::as_str), Some("1"));
        assert!(!env.contains_key("RUSTDOCFLAGS"));
        assert!(!env.contains_key("RUSTFLAGS"));
    }
}
