//! Pure Cargo command construction. No sandbox or toolchain operations.

use docsrs_metadata::Metadata;
use std::iter;

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--extern-html-root-takes-precedence",
];

pub(super) struct CommandArgs<'a> {
    docsrs_metadata: &'a Metadata,
    target: String,
    jobs: Option<usize>,
    // Explicit caller arguments, preserved in insertion order.
    cargo_args: Vec<String>,
    rustdoc_args: Vec<String>,
}

impl<'a> CommandArgs<'a> {
    pub(super) fn new(
        docsrs_metadata: &'a Metadata,
        target: impl Into<String>,
        jobs: Option<usize>,
    ) -> Self {
        Self {
            docsrs_metadata,
            target: target.into(),
            jobs,
            cargo_args: Vec::new(),
            rustdoc_args: Vec::new(),
        }
    }

    pub(super) fn target(&self) -> &str {
        &self.target
    }

    pub(super) fn cargo_arg(mut self, arg: impl Into<String>) -> Self {
        self.cargo_args.push(arg.into());
        self
    }

    pub(super) fn cargo_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.cargo_args.extend(args.into_iter().map(Into::into));
        self
    }

    pub(super) fn rustdoc_arg(mut self, arg: impl Into<String>) -> Self {
        self.rustdoc_args.push(arg.into());
        self
    }

    pub(super) fn rustdoc_args<S: Into<String>>(
        mut self,
        args: impl IntoIterator<Item = S>,
    ) -> Self {
        self.rustdoc_args.extend(args.into_iter().map(Into::into));
        self
    }

    pub(super) fn finish(&self) -> Vec<String> {
        let mut cargo_args = vec!["rustdoc".into(), "--lib".into(), "-Zrustdoc-map".into()];

        cargo_args.extend(self.feature_args());
        cargo_args.extend(self.rustc_config_args());
        cargo_args.extend(FlagConfig::BuildRustdocflags.args(self.rustdoc_flags()));
        cargo_args.extend(self.build_settings_args());

        // Preserve existing Cargo precedence: caller first, metadata last.
        cargo_args.extend_from_slice(&self.cargo_args);
        cargo_args.extend(filtered_metadata_args(&self.docsrs_metadata.cargo_args).cloned());
        cargo_args
    }

    fn rustdoc_flags(&self) -> impl Iterator<Item = &str> {
        // --cfg docsrs identifies docs.rs builds (rust-lang/docs.rs#2389).
        // Defaults → metadata → caller → unconditional flags.
        ["--cfg", "docsrs"]
            .into_iter()
            .chain(self.docsrs_metadata.rustdoc_args.iter().map(String::as_str))
            .chain(self.rustdoc_args.iter().map(String::as_str))
            .chain(UNCONDITIONAL_RUSTDOC_ARGS.iter().copied())
    }

    fn feature_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(features) = &self.docsrs_metadata.features {
            args.extend(["--features".into(), features.join(" ")]);
        }
        if self.docsrs_metadata.all_features {
            args.push("--all-features".into());
        }
        if self.docsrs_metadata.no_default_features {
            args.push("--no-default-features".into());
        }
        args
    }

    fn rustc_config_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.docsrs_metadata.rustc_args.is_empty() {
            return args;
        }
        args.extend(
            FlagConfig::BuildRustflags
                .args(self.docsrs_metadata.rustc_args.iter().map(String::as_str)),
        );
        if !self.docsrs_metadata.proc_macro {
            // Normal target builds also need these flags for host dependencies.
            // Proc-macro builds already use build.rustflags on the host; enabling
            // host config there would suppress build.rustdocflags.
            args.extend(["-Zhost-config".into(), "-Ztarget-applies-to-host".into()]);
            args.extend(
                FlagConfig::HostRustflags
                    .args(self.docsrs_metadata.rustc_args.iter().map(String::as_str)),
            );
        }
        args
    }

    fn build_settings_args(&self) -> Vec<String> {
        let target = &self.target;
        let mut args = vec![
            "--offline".into(),
            "-Zunstable-options".into(),
            format!(
                r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
            ),
        ];
        if let Some(jobs) = self.jobs {
            args.push(format!("-j{jobs}"));
        }
        // Proc-macro crates build for the host; --target can suppress their rustdoc flags.
        if !self.docsrs_metadata.proc_macro {
            args.extend(["--target".into(), self.target.clone()]);
        }
        args
    }
}

enum FlagConfig {
    BuildRustflags,
    HostRustflags,
    BuildRustdocflags,
}

impl FlagConfig {
    fn key(&self) -> &'static str {
        match self {
            Self::BuildRustflags => "build.rustflags",
            Self::HostRustflags => "host.rustflags",
            Self::BuildRustdocflags => "build.rustdocflags",
        }
    }

    fn args<S: Into<String>>(&self, flags: impl IntoIterator<Item = S>) -> [String; 2] {
        // TOML arrays preserve whitespace and quoting within individual arguments.
        let value = toml::Value::Array(
            flags
                .into_iter()
                .map(|flag| toml::Value::String(flag.into()))
                .collect(),
        );
        ["--config".into(), format!("{}={value}", self.key())]
    }
}

fn filtered_metadata_args<S>(args: impl IntoIterator<Item = S>) -> impl Iterator<Item = S>
where
    S: AsRef<str>,
{
    let mut args = args.into_iter().peekable();
    iter::from_fn(move || {
        loop {
            let next = args.next()?;
            let arg = next.as_ref();

            // We enable scraping ourselves where supported. The metadata option
            // would also reach JSON and coverage builds, which it breaks.
            if arg == "-Zrustdoc-scrape-examples" {
                continue;
            }
            if arg == "-Z"
                && args
                    .peek()
                    .is_some_and(|next| next.as_ref() == "rustdoc-scrape-examples")
            {
                args.next();
                continue;
            }

            return Some(next);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn cargo_args(
        target: &str,
        metadata: &Metadata,
        jobs: Option<usize>,
        cargo: Vec<String>,
        rustdoc: Vec<String>,
    ) -> Vec<String> {
        CommandArgs::new(metadata, target, jobs)
            .cargo_args(cargo)
            .rustdoc_args(rustdoc)
            .finish()
    }

    fn rustdoc_flags(args: &[String]) -> Vec<String> {
        let flag = FlagConfig::BuildRustdocflags;

        let config = args
            .iter()
            .find(|arg| arg.starts_with(&format!("{}=", flag.key())))
            .unwrap();

        let config: toml::Table = toml::from_str(config).unwrap();
        config["build"]["rustdocflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|flag| flag.as_str().unwrap().to_owned())
            .collect()
    }

    #[test_case(false)]
    #[test_case(true)]
    fn keeps_target_and_rustdoc_flags_correct_for_proc_macros(proc_macro: bool) {
        let mut metadata = Metadata::default();
        metadata.proc_macro = proc_macro;
        let args = cargo_args(
            "aarch64-unknown-linux-gnu",
            &metadata,
            Some(2),
            vec![],
            vec!["--output-format".into(), "json".into()],
        );
        assert!(args.starts_with(&["rustdoc".into(), "--lib".into()]));
        assert!(args.iter().any(|arg| arg == "--offline"));
        assert!(args.iter().any(|arg| arg == "-j2"));
        assert_eq!(
            args.windows(2)
                .any(|pair| pair == ["--target", "aarch64-unknown-linux-gnu"]),
            !proc_macro
        );
        if proc_macro {
            assert!(!args.iter().any(|arg| arg == "--target"));
        }
        assert!(args.iter().any(|arg| {
            arg.contains("https://docs.rs/{pkg_name}/{version}/aarch64-unknown-linux-gnu")
        }));
        let flags = rustdoc_flags(&args);
        assert!(flags.windows(2).any(|pair| pair == ["--cfg", "docsrs"]));
        assert!(
            flags
                .windows(2)
                .any(|pair| pair == ["--output-format", "json"])
        );
        assert!(
            flags
                .windows(2)
                .any(|pair| pair == ["--static-root-path", "/-/rustdoc.static/"])
        );
        assert!(
            flags
                .iter()
                .any(|flag| flag == "--extern-html-root-takes-precedence")
        );
    }

    #[test]
    fn metadata_lints_precede_caller_overrides() {
        let metadata: Metadata = r#"
[package]
name = "example"
[package.metadata.docs.rs]
rustdoc-args = ["-A", "rustdoc::invalid_html_tags", "-W", "missing_docs"]
"#
        .parse()
        .unwrap();
        let args = cargo_args(
            docsrs_metadata::HOST_TARGET,
            &metadata,
            None,
            vec![],
            vec!["-D".into(), "missing_docs".into()],
        );
        let mut expected = vec![
            "--cfg",
            "docsrs",
            "-A",
            "rustdoc::invalid_html_tags",
            "-W",
            "missing_docs",
            "-Z",
            "unstable-options",
            "-D",
            "missing_docs",
        ];
        expected.extend(UNCONDITIONAL_RUSTDOC_ARGS);
        assert_eq!(rustdoc_flags(&args), expected);
    }

    #[test]
    fn preserves_metadata_features_custom_flags_and_extra_cargo_arguments() {
        let metadata: Metadata = r#"
[package]
name = "example"
[package.metadata.docs.rs]
features = ["extra", "another"]
all-features = true
no-default-features = true
rustdoc-args = ["--cfg", "custom_docs"]
rustc-args = ["--cfg", "custom_build"]
cargo-args = ["--verbose"]
"#
        .parse()
        .unwrap();
        let args = cargo_args(
            docsrs_metadata::HOST_TARGET,
            &metadata,
            None,
            vec!["--locked".into()],
            vec![],
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--features", "extra another"])
        );
        for flag in [
            "--all-features",
            "--no-default-features",
            "--verbose",
            "--locked",
        ] {
            assert!(args.iter().any(|arg| arg == flag), "missing {flag}");
        }
        assert!(!args.iter().any(|arg| arg.starts_with("-j")));
        let flags = rustdoc_flags(&args);
        assert!(
            flags
                .windows(2)
                .any(|pair| pair == ["--cfg", "custom_docs"])
        );
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("build.rustflags=") && arg.contains("custom_build"))
        );
    }
    #[test_case(None)]
    #[test_case(Some(vec![]))]
    #[test_case(Some(vec!["one".into()]))]
    #[test_case(Some(vec!["one".into(), "two".into()]))]
    fn preserves_feature_selection(features: Option<Vec<String>>) {
        for (all_features, no_default_features) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut metadata = Metadata::default();
            metadata.features = features.clone();
            metadata.all_features = all_features;
            metadata.no_default_features = no_default_features;
            let args = CommandArgs::new(&metadata, "target", None).finish();
            let feature_position = args.iter().position(|arg| arg == "--features");
            assert_eq!(
                feature_position.map(|index| &args[index + 1]),
                features.as_ref().map(|names| names.join(" ")).as_ref()
            );
            assert_eq!(args.iter().any(|arg| arg == "--all-features"), all_features);
            assert_eq!(
                args.iter().any(|arg| arg == "--no-default-features"),
                no_default_features
            );
        }
    }

    #[test_case(false)]
    #[test_case(true)]
    fn preserves_rustc_and_json_flags_for_each_crate_type(proc_macro: bool) {
        let mut metadata = Metadata::default();
        metadata.proc_macro = proc_macro;
        metadata.rustc_args = vec!["--cfg".into(), r#"label="a value with spaces""#.into()];
        metadata.rustdoc_args = vec!["--cfg".into(), "custom_docs".into()];
        let args = CommandArgs::new(&metadata, "target", None)
            .rustdoc_args(["--output-format", "json"])
            .finish();
        assert_eq!(args.iter().any(|arg| arg == "--target"), !proc_macro);
        let mut expected_rustdoc = vec![
            "--cfg",
            "docsrs",
            "--cfg",
            "custom_docs",
            "--output-format",
            "json",
        ];
        expected_rustdoc.extend(UNCONDITIONAL_RUSTDOC_ARGS);
        assert_eq!(rustdoc_flags(&args), expected_rustdoc);
        let config: toml::Table = toml::from_str(
            args.iter()
                .find(|arg| arg.starts_with("build.rustflags="))
                .unwrap(),
        )
        .unwrap();
        let expected = toml::Value::try_from(&metadata.rustc_args).unwrap();
        assert_eq!(config["build"]["rustflags"], expected);
        assert_eq!(args.iter().any(|arg| arg == "-Zhost-config"), !proc_macro);
        assert_eq!(
            args.iter().any(|arg| arg == "-Ztarget-applies-to-host"),
            !proc_macro
        );
        let host = args.iter().find(|arg| arg.starts_with("host.rustflags="));
        assert_eq!(host.is_some(), !proc_macro);
        if let Some(host) = host {
            let config: toml::Table = toml::from_str(host).unwrap();
            assert_eq!(config["host"]["rustflags"], expected);
        }
    }

    #[test_case(vec!["-Zrustdoc-scrape-examples"])]
    #[test_case(vec!["-Z", "rustdoc-scrape-examples"])]
    fn filters_scraping_and_preserves_cargo_argument_order(scrape_args: Vec<&str>) {
        let mut metadata = Metadata::default();
        metadata.cargo_args = scrape_args.into_iter().map(String::from).collect();
        metadata.cargo_args.extend([
            "-Zbuild-std".into(),
            "-Z".into(),
            "build-std".into(),
            "--config=build.jobs=3".into(),
            "-Z".into(),
        ]);
        let args = CommandArgs::new(&metadata, "target", Some(1))
            .cargo_arg("--config=build.jobs=2")
            .cargo_args(["--locked"])
            .finish();
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("rustdoc-scrape-examples"))
        );
        assert!(args.ends_with(&[
            "-j1".into(),
            "--target".into(),
            "target".into(),
            "--config=build.jobs=2".into(),
            "--locked".into(),
            "-Zbuild-std".into(),
            "-Z".into(),
            "build-std".into(),
            "--config=build.jobs=3".into(),
            "-Z".into(),
        ]));
    }

    #[test_case(vec!["-Zrustdoc-scrape-examples"])]
    #[test_case(vec!["-Z", "rustdoc-scrape-examples"])]
    fn preserves_caller_scraping_while_filtering_metadata(scrape_args: Vec<&str>) {
        let mut metadata = Metadata::default();
        let mut expected = CommandArgs::new(&metadata, "target", None).finish();
        expected.extend(scrape_args.iter().copied().map(String::from));
        expected.push("--locked".into());

        metadata.cargo_args = scrape_args.iter().copied().map(String::from).collect();
        metadata.cargo_args.push("--locked".into());
        let args = CommandArgs::new(&metadata, "target", None)
            .cargo_args(scrape_args)
            .finish();

        assert_eq!(args, expected);
    }

    #[test]
    fn default_command_and_caller_rustdoc_flags_keep_their_order() {
        let args = CommandArgs::new(&Metadata::default(), "target", None)
            .rustdoc_arg("--cfg")
            .rustdoc_args([r#"label="a value with spaces""#])
            .finish();
        assert_eq!(
            &args[..4],
            ["rustdoc", "--lib", "-Zrustdoc-map", "--config"]
        );
        assert_eq!(
            &args[5..],
            [
                "--offline",
                "-Zunstable-options",
                r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{pkg_name}/{version}/target""#,
                "--target",
                "target",
            ]
        );
        let mut expected = vec!["--cfg", "docsrs", "--cfg", r#"label="a value with spaces""#];
        expected.extend(UNCONDITIONAL_RUSTDOC_ARGS);
        assert_eq!(rustdoc_flags(&args), expected);
    }
}
