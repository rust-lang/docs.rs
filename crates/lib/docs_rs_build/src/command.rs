use crate::build::ReleaseBuild;
use anyhow::{Context as _, Result};
use docsrs_metadata::{DEFAULT_TARGETS, Metadata};
use rustwide::cmd::Command;

const UNCONDITIONAL_RUSTDOC_ARGS: &[&str] = &[
    "--static-root-path",
    "/-/rustdoc.static/",
    "--cap-lints",
    "warn",
    "--extern-html-root-takes-precedence",
];

#[must_use = "call `.prepare()` to create and prepare the rustwide command"]
pub struct PrepareCommand<'release_build, 'build, 'ws> {
    release_build: &'release_build ReleaseBuild<'build, 'ws>,

    target: String,

    /// Extra arguments passed to Cargo before rustdoc's argument separator.
    cargo_args: Vec<String>,

    /// Extra rustdoc flags, such as the HTML or JSON output mode.
    rustdoc_args: Vec<String>,
}

impl<'release_build, 'build, 'ws> PrepareCommand<'release_build, 'build, 'ws> {
    pub(crate) fn new(
        release_build: &'release_build ReleaseBuild<'build, 'ws>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            release_build,
            target: target.into(),
            cargo_args: Vec::new(),
            rustdoc_args: Vec::new(),
        }
    }

    pub fn cargo_arg(mut self, arg: impl Into<String>) -> Self {
        self.cargo_args.push(arg.into());
        self
    }

    pub fn cargo_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.cargo_args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn rustdoc_arg(mut self, arg: impl Into<String>) -> Self {
        self.rustdoc_args.push(arg.into());
        self
    }

    pub fn rustdoc_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.rustdoc_args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn prepare<'pl>(self) -> Result<Command<'ws, 'pl>> {
        let cargo_args = cargo_args(
            &self.target,
            &self.release_build.metadata,
            self.release_build.environment.cargo_jobs(),
            self.cargo_args,
            self.rustdoc_args,
        );

        if uses_build_std(&cargo_args) {
            self.release_build
                .fetch_build_std_dependencies([self.target.as_ref()])
                .context("error fetching build_std dependencies")?;
        } else if !DEFAULT_TARGETS.contains(&self.target.as_ref()) {
            self.release_build
                .environment
                .configured_toolchain()
                .add_target(self.release_build.environment.workspace(), &self.target)
                .context("error adding non-default target to toolchain")?;
        }

        let mut command = self
            .release_build
            .build
            .cargo()
            .timeout(Some(self.release_build.limits.timeout()))
            .no_output_timeout(None);

        for (key, value) in self.release_build.metadata.environment_variables() {
            command = command.env(key, value);
        }
        Ok(command.args(cargo_args))
    }
}

fn uses_build_std<S: AsRef<str>>(args: impl IntoIterator<Item = S>) -> bool {
    let mut args = args.into_iter().map(|arg| arg.as_ref()).peekable();

    while let Some(arg) = args.next() {
        if arg.starts_with("-Zbuild-std") {
            return true;
        }

        if arg == "-Z"
            && args
                .peek()
                .is_some_and(|next| next.starts_with("build-std"))
        {
            return true;
        }
    }

    false
}

fn cargo_args(
    target: &str,
    metadata: &Metadata,
    cargo_jobs: Option<usize>,
    cargo_args: Vec<String>,
    mut rustdoc_args: Vec<String>,
) -> Vec<String> {
    let mut additional_args: Vec<String> = vec![
        "--offline".into(),
        "-Zunstable-options".into(),
        format!(
            r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
        ),
    ];

    if let Some(jobs) = cargo_jobs {
        additional_args.push(format!("-j{jobs}"));
    }

    // Cargo puts proc-macro documentation in the host target directory and
    // does not reliably forward RUSTDOCFLAGS when --target is supplied.
    if !metadata.proc_macro {
        additional_args.push("--target".into());
        additional_args.push(target.into());
    }

    additional_args.extend(cargo_args);

    rustdoc_args.extend(UNCONDITIONAL_RUSTDOC_ARGS.iter().map(|arg| (*arg).into()));
    metadata.cargo_args(&additional_args, &rustdoc_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_build_std_spellings() {
        assert!(uses_build_std(["-Zbuild-std=core"]));
        assert!(uses_build_std(["-Z", "build-std"]));
        assert!(!uses_build_std(["-Zunstable-options"]));
    }
}
