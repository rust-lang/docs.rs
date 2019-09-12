use error::Result;
use log::LevelFilter;
use rustwide::{
    cmd::{Command, SandboxBuilder},
    logging::{self, LogStorage},
    Crate, Toolchain, Workspace, WorkspaceBuilder,
};
use std::path::Path;
use utils::cargo_metadata::CargoMetadata;
use utils::parse_rustc_version;
use Metadata;

// TODO: 1GB might not be enough
const SANDBOX_MEMORY_LIMIT: usize = 1024 * 1024 * 1024; // 1GB
const SANDBOX_NETWORKING: bool = false;
const SANDBOX_MAX_LOG_SIZE: usize = 1024 * 1024; // 1MB
const SANDBOX_MAX_LOG_LINES: usize = 10_000;

pub fn build_doc_rustwide(
    name: &str,
    version: &str,
    target: Option<&str>,
) -> Result<BuildDocOutput> {
    // TODO: Handle workspace path correctly
    let workspace = WorkspaceBuilder::new(Path::new("/tmp/docs-builder"), "docsrs").init()?;

    // TODO: Instead of using just nightly, we can pin a version.
    //       Docs.rs can only use nightly (due to unstable docs.rs features in rustdoc)
    let toolchain = Toolchain::Dist {
        name: "nightly".into(),
    };
    toolchain.install(&workspace)?;
    if let Some(target) = target {
        toolchain.add_target(&workspace, target)?;
    }

    let krate = Crate::crates_io(name, version);
    krate.fetch(&workspace)?;

    let sandbox = SandboxBuilder::new()
        .memory_limit(Some(SANDBOX_MEMORY_LIMIT))
        .enable_networking(SANDBOX_NETWORKING);

    let mut build_dir = workspace.build_dir(&format!("{}-{}", name, version));
    let pkg = build_dir.build(&toolchain, &krate, sandbox, |build| {
        let metadata = Metadata::from_source_dir(&build.host_source_dir())?;
        let cargo_metadata = CargoMetadata::load(&workspace, &toolchain, &build.host_source_dir())?;

        let mut rustdoc_flags: Vec<String> = vec![
            "-Z".to_string(),
            "unstable-options".to_string(),
            "--resource-suffix".to_string(),
            format!(
                "-{}",
                parse_rustc_version(rustc_version(&workspace, &toolchain)?)?
            ),
            "--static-root-path".to_string(),
            "/".to_string(),
            "--disable-per-crate-search".to_string(),
        ];

        for dep in &cargo_metadata.root_dependencies() {
            rustdoc_flags.push("--extern-html-root-url".to_string());
            rustdoc_flags.push(format!(
                "{}=https://docs.rs/{}/{}",
                dep.name.replace("-", "_"),
                dep.name,
                dep.version
            ));
        }

        let mut cargo_args = vec!["doc".to_owned(), "--lib".to_owned(), "--no-deps".to_owned()];
        if let Some(features) = &metadata.features {
            cargo_args.push("--features".to_owned());
            cargo_args.push(features.join(" "));
        }
        if metadata.all_features {
            cargo_args.push("--all-features".to_owned());
        }
        if metadata.no_default_features {
            cargo_args.push("--no-default-features".to_owned());
        }
        if let Some(target) = target {
            cargo_args.push("--target".into());
            cargo_args.push(target.into());
        }

        let mut storage = LogStorage::new(LevelFilter::Info);
        storage.set_max_size(SANDBOX_MAX_LOG_SIZE);
        storage.set_max_lines(SANDBOX_MAX_LOG_LINES);

        logging::capture(&storage, || {
            build
                .cargo()
                .env(
                    "RUSTFLAGS",
                    metadata
                        .rustc_args
                        .map(|args| args.join(""))
                        .unwrap_or("".to_owned()),
                )
                .env("RUSTDOCFLAGS", rustdoc_flags.join(" "))
                .args(&cargo_args)
                .run()
        })?;

        // TODO: We need to return build result as well
        Ok(BuildDocOutput {
            package_version: cargo_metadata.root().version.to_string(),
            build_log: storage.to_string(),
        })
    })?;

    Ok(pkg)
}

pub struct BuildDocOutput {
    package_version: String,
    build_log: String,
}

impl BuildDocOutput {
    pub fn package_version(&self) -> &str {
        &self.package_version
    }

    pub fn build_log(&self) -> &str {
        &self.build_log
    }
}

fn rustc_version(workspace: &Workspace, toolchain: &Toolchain) -> Result<String> {
    let res = Command::new(workspace, toolchain.rustc())
        .args(&["--version"])
        .log_output(false)
        .run_capture()?;

    let mut iter = res.stdout_lines().iter();
    if let (Some(line), None) = (iter.next(), iter.next()) {
        Ok(line.clone())
    } else {
        Err(::failure::err_msg(
            "invalid output returned by `rustc --version`",
        ))
    }
}
