use cargo::core::{enable_nightly_features, Package, SourceId, Workspace};
use cargo::sources::SourceConfigMap;
use cargo::util::{internal, Config};
use error::Result;
use rustwide::{cmd::SandboxBuilder, Crate, Toolchain, WorkspaceBuilder};
use std::collections::HashSet;
use std::path::Path;
use utils::{get_current_versions, parse_rustc_version, resolve_deps};
use Metadata;

pub fn build_doc_rustwide(name: &str, version: &str, target: Option<&str>) -> Result<Package> {
    // TODO: Handle workspace path correctly
    let rustwide_workspace =
        WorkspaceBuilder::new(Path::new("/tmp/docs-builder"), "docsrs").init()?;

    // TODO: Instead of using just nightly, we can pin a version.
    //       Docs.rs can only use nightly (due to unstable docs.rs features in rustdoc)
    let toolchain = Toolchain::Dist {
        name: "nightly".into(),
    };
    toolchain.install(&rustwide_workspace)?;

    let krate = Crate::crates_io(name, version);
    krate.fetch(&rustwide_workspace)?;

    // Configure a sandbox with 1GB of RAM and no network access
    // TODO: 1GB might not be enough
    let sandbox = SandboxBuilder::new()
        .memory_limit(Some(1024 * 1024 * 1024))
        .enable_networking(false);

    let mut build_dir = rustwide_workspace.build_dir(&format!("{}-{}", name, version));
    let pkg = build_dir.build(&toolchain, &krate, sandbox, |build| {
        enable_nightly_features();
        let config = Config::default()?;
        let source_id = try!(SourceId::crates_io(&config));
        let source_cfg_map = try!(SourceConfigMap::new(&config));
        let manifest_path = build.host_source_dir().join("Cargo.toml");
        let ws = Workspace::new(&manifest_path, &config)?;
        let pkg = ws.load(&manifest_path)?;

        let metadata = Metadata::from_package(&pkg).map_err(|e| internal(e.to_string()))?;

        let mut rustdoc_flags: Vec<String> = vec![
            "-Z".to_string(),
            "unstable-options".to_string(),
            "--resource-suffix".to_string(),
            // FIXME: We need to get rustc version inside of container.
            //        Our get_current_versions gets rustc version from host system not container.
            format!("-{}", parse_rustc_version(get_current_versions()?.0)?),
            "--static-root-path".to_string(),
            "/".to_string(),
            "--disable-per-crate-search".to_string(),
        ];

        let source = try!(source_cfg_map.load(source_id, &HashSet::new()));
        let _lock = try!(config.acquire_package_cache_lock());

        for (name, dep) in try!(resolve_deps(&pkg, &config, source)) {
            rustdoc_flags.push("--extern-html-root-url".to_string());
            rustdoc_flags.push(format!(
                "{}=https://docs.rs/{}/{}",
                name.replace("-", "_"),
                dep.name(),
                dep.version()
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

        // TODO: We need to use build result here
        // FIXME: We also need build log (basically stderr message)
        let result = build
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
            .run();

        // TODO: We need to return build result as well
        Ok(pkg)
    })?;

    Ok(pkg)
}
