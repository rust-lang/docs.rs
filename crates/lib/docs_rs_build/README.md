# docs_rs_build

`docs_rs_build` contains the service-independent part of the docs.rs build
pipeline. It configures rustwide, applies the docs.rs sandbox limits and Cargo
arguments, reads the crate's docs.rs metadata, and runs all build steps for a
release in one sandbox.

The crate does not store build results in the docs.rs database or copy artifacts
to docs.rs storage. A caller can decide what to do with the returned paths,
logs, coverage, and sandbox statistics.

## Workspace lifecycle

`BuildEnvironment` retains the configuration needed to recreate its rustwide
workspace. Long-running builders should call
`refresh_workspace_if_interval_passed` between releases:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::{BuildEnvironment, SandboxImageSource};
# use std::{path::Path, time::Duration};
# fn main() -> Result<()> {
let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
    .sandbox_image(SandboxImageSource::Remote(
        "docsrs/build-env:latest".into(),
    ))
    .workspace_reinitialization_interval(Duration::from_secs(24 * 60 * 60))
    .build()?;

environment.refresh_workspace_if_interval_passed()?;
# Ok(())
# }
```

`Remote` pulls the configured image on every initialization, including a timed
refresh. `LocalOrRemote` uses an existing local image and only pulls when it is
missing, which is useful for locally built images. Workspace initialization and
refresh both purge stale build directories; caches can be removed explicitly
with `purge_caches`.

## Toolchain lifecycle

Before accepting builds, and periodically in a long-running builder, update the
configured toolchain. The caller decides whether a compiler change requires
regenerating and publishing shared rustdoc files:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::BuildEnvironment;
# use rustwide::Toolchain;
# use std::path::Path;
# fn main() -> Result<()> {
let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace")).build()?;

// A service can fetch this selection from its configuration or database.
environment.set_toolchain(Toolchain::dist("nightly"))?;
if environment.update_toolchain()? {
    let essential_files = environment.build_essential_files()?;
    // Inspect or publish essential_files.into_inner() here.
#   let _ = essential_files;
}
# Ok(())
# }
```

Preparation installs the toolchain, the docs.rs default targets, and the
`llvm-tools-preview`, `rustc-dev`, and `rustfmt` components. Non-default targets
left by individual crate builds are removed before a distribution toolchain is
updated. CI toolchains are installed and treated as changed on every update.

A durable service should additionally compare `rustc_version()` with the
version of the essential files it last published. That ensures generation and
publication are retried after a failure even when the installed compiler no
longer changes on the next update check. The published version should only be
recorded after publication succeeds.

## Host resources and compiler metrics

Before fetching a release, the environment verifies that the host's currently
available memory can satisfy the release's effective sandbox limit. This check
is enabled by default and can be disabled when the caller intentionally wants
the sandbox or host runtime to enforce the limit:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::BuildEnvironment;
# use std::path::Path;
# fn main() -> Result<()> {
let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
    .validate_host_resources(false)
    .build()?;
# let _ = environment;
# Ok(())
# }
```

Compiler metrics are enabled for all HTML builds when an environment-wide
destination is configured:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::BuildEnvironment;
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
    .compiler_metrics_collection_path("./compiler-metrics")
    .build()?;
let krate = Crate::crates_io("serde", "1.0.219");

let result = environment.release(&krate).run(|build| build.build_docs())?;
# let _ = result;
# Ok(())
# }
```

For HTML builds, the library passes rustdoc's unstable metrics directory flag
and copies the generated files out of rustwide's target directory before the
release sandbox is cleaned up.

## Complete release build

The usual entry point builds coverage, rustdoc JSON, and HTML documentation for
the default target and the additional targets selected by the crate's metadata:

```rust,no_run
use anyhow::Result;
use docs_rs_build::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::path::Path;

fn main() -> Result<()> {
    let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
        .sandbox_image(SandboxImageSource::LocalOrRemote(
            "docsrs/build-env:latest".into(),
        ))
        .build()?;

    let krate = Crate::crates_io("serde", "1.0.219");
    let build = environment
        .release(&krate)
        .run(|build| build.build_docs())?;

    println!("sandbox statistics: {:#?}", build.statistics());
    let release = build.into_inner();
    for target in release.targets {
        println!(
            "{}: documentation succeeded: {}",
            target.target,
            target.documentation.successful()
        );
    }

    Ok(())
}
```

See [`examples/full_release.rs`](examples/full_release.rs) for a complete
command-line version.

## Selecting individual build products

`ReleaseContext::run` prepares and fetches the release once, then gives the
callback a `ReleaseBuild`. Calls made inside that callback share the same
rustwide build and sandbox:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::{BuildEnvironment, SandboxImageSource};
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
#     .sandbox_image(SandboxImageSource::LocalOrRemote("docsrs/build-env:latest".into()))
#     .build()?;
# let krate = Crate::crates_io("serde", "1.0.219");
let selected = environment.release(&krate).run(|build| {
    let target = build.selected_targets().default_target.to_owned();
    let json = build.build_rustdoc_json(&target);
    let documentation = build.build_documentation(&target);
    Ok((json, documentation))
})?;
# let _ = selected;
# Ok(())
# }
```

See [`examples/custom_build.rs`](examples/custom_build.rs).

## Archiving sources before a build

`ReleaseContext::fetch` exposes an intermediate phase for callers that need to
archive sources before metadata parsing or sandbox preparation:

```rust,no_run
# use anyhow::Result;
# use docs_rs_build::BuildEnvironment;
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace")).build()?;
let krate = Crate::crates_io("serde", "1.0.219");
let result = environment
    .release(&krate)
    .fetch()?
    .try_inspect(|fetched| fetched.copy_source_to("./source-archive-input"))?
    .run(|build| build.build_docs())?;
# let _ = result;
# Ok(())
# }
```

Callers that do not need an intermediate source step can continue using
`release().run(...)`, which performs the fetch automatically.

See [`examples/extract_sources.rs`](examples/extract_sources.rs) for a runnable
example that extracts sources before entering build preparation.

`running_inside_docker(true)` is only needed when the calling program itself is
inside a container, for example a container action using the host Docker socket.
Leave it at its default (`false`) when invoking the program directly on a host.
