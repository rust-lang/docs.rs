use std::path::Path;
use toml::Value;
use error::Result;
use failure::err_msg;

/// Metadata for custom builds
///
/// You can customize docs.rs builds by defining `[package.metadata.docs.rs]` table in your
/// crates' `Cargo.toml`.
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
/// extra-targets = [ "x86_64-apple-darwin", "x86_64-pc-windows-msvc" ]
/// rustc-args = [ "--example-rustc-arg" ]
/// rustdoc-args = [ "--example-rustdoc-arg" ]
/// ```
///
/// You can define one or more fields in your `Cargo.toml`.
pub struct Metadata {
    /// List of features docs.rs will build.
    ///
    /// By default, docs.rs will only build default features.
    pub features: Option<Vec<String>>,

    /// Set `all-features` to true if you want docs.rs to build all features for your crate
    pub all_features: bool,

    /// Docs.rs will always build default features.
    ///
    /// Set `no-default-fatures` to `false` if you want to build only certain features.
    pub no_default_features: bool,

    /// Docs.rs is running on `x86_64-unknown-linux-gnu` target system and default documentation
    /// is always built on this target. You can change default target by setting this.
    pub default_target: Option<String>,

    /// If you want a crate to build only for specific targets,
    /// set `extra-targets` to the list of targets to build, in addition to `default-target`.
    ///
    /// If you do not set `extra_targets`, all of the tier 1 supported targets will be built.
    /// If you set `extra_targets` to an empty array, only the default target will be built.
    pub extra_targets: Option<Vec<String>>,

    /// List of command line arguments for `rustc`.
    pub rustc_args: Option<Vec<String>>,

    /// List of command line arguments for `rustdoc`.
    pub rustdoc_args: Option<Vec<String>>,
}



impl Metadata {
    pub(crate) fn from_source_dir(source_dir: &Path) -> Result<Metadata> {
        for c in ["Cargo.toml.orig", "Cargo.toml"].iter() {
            let manifest_path = source_dir.clone().join(c);
            if manifest_path.exists() {
                return Ok(Metadata::from_manifest(manifest_path));
            }
        }
        Err(err_msg("Manifest not found"))
    }

    fn from_manifest<P: AsRef<Path>>(path: P) -> Metadata {
        use std::fs::File;
        use std::io::Read;
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(_) => return Metadata::default(),
        };
        let mut s = String::new();
        if let Err(_) = f.read_to_string(&mut s) {
            return Metadata::default();
        }
        Metadata::from_str(&s)
    }


    // This is similar to Default trait but it's private
    fn default() -> Metadata {
        Metadata {
            features: None,
            all_features: false,
            no_default_features: false,
            default_target: None,
            rustc_args: None,
            rustdoc_args: None,
            extra_targets: None,
        }
    }


    fn from_str(manifest: &str) -> Metadata {
        let mut metadata = Metadata::default();

        let manifest = match manifest.parse::<Value>() {
            Ok(m) => m,
            Err(_) => return metadata,
        };

        if let Some(table) = manifest.get("package").and_then(|p| p.as_table())
            .and_then(|p| p.get("metadata")).and_then(|p| p.as_table())
                .and_then(|p| p.get("docs")).and_then(|p| p.as_table())
                .and_then(|p| p.get("rs")).and_then(|p| p.as_table()) {
                    metadata.features = table.get("features").and_then(|f| f.as_array())
                        .and_then(|f| f.iter().map(|v| v.as_str().map(|v| v.to_owned())).collect());
                    metadata.no_default_features = table.get("no-default-features")
                        .and_then(|v| v.as_bool()).unwrap_or(metadata.no_default_features);
                    metadata.all_features = table.get("all-features")
                        .and_then(|v| v.as_bool()).unwrap_or(metadata.all_features);
                    metadata.default_target = table.get("default-target")
                        .and_then(|v| v.as_str()).map(|v| v.to_owned());
                    metadata.extra_targets = table.get("extra-targets").and_then(|f| f.as_array())
                        .and_then(|f| f.iter().map(|v| v.as_str().map(|v| v.to_owned())).collect());
                    metadata.rustc_args = table.get("rustc-args").and_then(|f| f.as_array())
                        .and_then(|f| f.iter().map(|v| v.as_str().map(|v| v.to_owned())).collect());
                    metadata.rustdoc_args = table.get("rustdoc-args").and_then(|f| f.as_array())
                        .and_then(|f| f.iter().map(|v| v.as_str().map(|v| v.to_owned())).collect());
                }

        metadata
    }
    // Return (default_target, all other targets that should be built with duplicates removed)
    pub(super) fn targets(&self) -> (&str, Vec<&str>) {
        use super::rustwide_builder::{HOST_TARGET, TARGETS};
        // Let people opt-in to only having specific targets
        // Ideally this would use Iterator instead of Vec so I could collect to a `HashSet`,
        // but I had trouble with `chain()` ¯\_(ツ)_/¯
        let mut all_targets: Vec<_> = self.default_target.as_deref().into_iter().collect();
        match &self.extra_targets {
            Some(targets) => all_targets.extend(targets.iter().map(|s| s.as_str())),
            None if all_targets.is_empty() => {
                // Make sure HOST_TARGET is first
                all_targets.push(HOST_TARGET);
                all_targets.extend(TARGETS.iter().copied().filter(|&t| t != HOST_TARGET));
            }
            None => all_targets.extend(TARGETS.iter().copied()),
        };

        // default_target unset and extra_targets set to `[]`
        let landing_page = if all_targets.is_empty() {
            HOST_TARGET
        } else {
            // This `swap_remove` has to come before the `sort()` to keep the ordering
            // `swap_remove` is ok because ordering doesn't matter except for first element
            all_targets.swap_remove(0)
        };
        // Remove duplicates
        all_targets.sort();
        all_targets.dedup();
        // Remove landing_page so we don't build it twice.
        // It wasn't removed during dedup because we called `swap_remove()` first.
        if let Ok(index) = all_targets.binary_search(&landing_page) {
            all_targets.swap_remove(index);
        }
        (landing_page, all_targets)
    }
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::Metadata;

    #[test]
    fn test_cratesfyi_metadata() {
        let _ = env_logger::try_init();
        let manifest = r#"
            [package]
            name = "test"

            [package.metadata.docs.rs]
            features = [ "feature1", "feature2" ]
            all-features = true
            no-default-features = true
            default-target = "x86_64-unknown-linux-gnu"
            extra-targets = [ "x86_64-apple-darwin", "x86_64-pc-windows-msvc" ]
            rustc-args = [ "--example-rustc-arg" ]
            rustdoc-args = [ "--example-rustdoc-arg" ]
        "#;

        let metadata = Metadata::from_str(manifest);

        assert!(metadata.features.is_some());
        assert!(metadata.all_features == true);
        assert!(metadata.no_default_features == true);
        assert!(metadata.default_target.is_some());
        assert!(metadata.rustdoc_args.is_some());

        let features = metadata.features.unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[0], "feature1".to_owned());
        assert_eq!(features[1], "feature2".to_owned());

        assert_eq!(metadata.default_target.unwrap(), "x86_64-unknown-linux-gnu".to_owned());

        let extra_targets = metadata.extra_targets.expect("should have explicit extra target");
        assert_eq!(extra_targets.len(), 2);
        assert_eq!(extra_targets[0], "x86_64-apple-darwin");
        assert_eq!(extra_targets[1], "x86_64-pc-windows-msvc");

        let rustc_args = metadata.rustc_args.unwrap();
        assert_eq!(rustc_args.len(), 1);
        assert_eq!(rustc_args[0], "--example-rustc-arg".to_owned());

        let rustdoc_args = metadata.rustdoc_args.unwrap();
        assert_eq!(rustdoc_args.len(), 1);
        assert_eq!(rustdoc_args[0], "--example-rustdoc-arg".to_owned());
    }

    #[test]
    fn test_no_extra_targets() {
        // metadata section but no extra_targets
        let manifest = r#"
            [package]
            name = "test"

            [package.metadata.docs.rs]
            features = [ "feature1", "feature2" ]
        "#;
        let metadata = Metadata::from_str(manifest);
        assert!(metadata.extra_targets.is_none());

        // no package.metadata.docs.rs section
        let metadata = Metadata::from_str(r#"
            [package]
            name = "test"
        "#);
        assert!(metadata.extra_targets.is_none());

        // extra targets explicitly set to empty array
        let metadata = Metadata::from_str(r#"
            [package.metadata.docs.rs]
            extra-targets = []
        "#);
        assert!(metadata.extra_targets.unwrap().is_empty());
    }
    #[test]
    fn test_select_extra_targets() {
        use crate::docbuilder::rustwide_builder::{HOST_TARGET, TARGETS};

        let mut metadata = Metadata::default();
        // unchanged default_target, extra targets not specified
        let (default, tier_one) = metadata.targets();
        assert_eq!(default, HOST_TARGET);
        // should be equal to TARGETS \ {HOST_TARGET}
        for actual in &tier_one {
            assert!(TARGETS.contains(actual));
        }
        for expected in TARGETS {
            if *expected == HOST_TARGET {
                assert!(!tier_one.contains(&HOST_TARGET));
            } else {
                assert!(tier_one.contains(expected));
            }
        }

        // unchanged default_target, extra targets specified to be empty
        metadata.extra_targets = Some(Vec::new());
        let (default, others) = metadata.targets();
        assert_eq!(default, HOST_TARGET);
        assert!(others.is_empty());

        // unchanged default_target, extra targets non-empty
        metadata.extra_targets = Some(vec!["i686-pc-windows-msvc".into(), "i686-apple-darwin".into()]);
        let (default, others) = metadata.targets();
        assert_eq!(default, "i686-pc-windows-msvc");
        assert_eq!(others.len(), 1);
        assert!(others.contains(&"i686-apple-darwin"));

        // make sure that default_target is not built twice
        metadata.extra_targets = Some(vec![HOST_TARGET.into()]);
        let (default, others) = metadata.targets();
        assert_eq!(default, HOST_TARGET);
        assert!(others.is_empty());

        // make sure that duplicates are removed
        metadata.extra_targets = Some(vec!["i686-pc-windows-msvc".into(), "i686-pc-windows-msvc".into()]);
        let (default, others) = metadata.targets();
        assert_eq!(default, "i686-pc-windows-msvc");
        assert!(others.is_empty());

        // make sure that `default_target` always takes priority over `targets`
        metadata.default_target = Some("i686-apple-darwin".into());
        let (default, others) = metadata.targets();
        assert_eq!(default, "i686-apple-darwin");
        assert_eq!(others.len(), 1);
        assert!(others.contains(&"i686-pc-windows-msvc"));

        // make sure that `default_target` takes priority over `HOST_TARGET`
        metadata.extra_targets = Some(vec![]);
        let (default, others) = metadata.targets();
        assert_eq!(default, "i686-apple-darwin");
        assert!(others.is_empty());

        // and if `extra_targets` is unset, it should still be set to `TARGETS`
        metadata.extra_targets = None;
        let (default, others) = metadata.targets();
        assert_eq!(default, "i686-apple-darwin");
        let tier_one_targets_no_default: Vec<_> = TARGETS.iter().filter(|&&t| t != "i686-apple-darwin").copied().collect();
        assert_eq!(others, tier_one_targets_no_default);

    }
}
