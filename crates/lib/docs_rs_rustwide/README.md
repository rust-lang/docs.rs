# docs_rs_rustwide

`docs_rs_rustwide` contains the service-independent part of the docs.rs build
pipeline. It configures rustwide, applies the docs.rs sandbox limits and Cargo
arguments, reads the crate's docs.rs metadata, and runs all build steps for a
release in one sandbox.

Initialize `docs_rs_rustwide::logging::init(log_build_logs)` once before running
builds. This enables log capture; `true` also forwards build output to the
application's tracing subscriber.

The crate does not store build results in the docs.rs database or copy artifacts
to docs.rs storage. A caller can decide what to do with the returned paths,
logs, coverage, and sandbox statistics.

## Workspace lifecycle

`BuildEnvironment` retains the configuration needed to recreate its rustwide
workspace. Long-running builders should call
`perform_maintenance` between releases:

```rust,no_run
# use anyhow::Result;
# use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
# use std::{path::Path, time::Duration};
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
    .sandbox_image(SandboxImageSource::Remote(
        "ghcr.io/rust-lang/crates-build-env/linux".into(),
    ))
    .workspace_reinitialization_interval(Duration::from_secs(24 * 60 * 60))
    .build()?;

let maintenance = environment.perform_maintenance()?;
// Publish new shared rustdoc files when maintenance.toolchain_updated is true.
# let _ = maintenance;
# Ok(())
# }
```

`Remote` pulls the configured image on every initialization, including a timed
refresh. `LocalOrRemote` uses an existing local image and only pulls when it is
missing, which is useful for locally built images. Workspace initialization and
refresh both purge stale build directories. Toolchain changes automatically
purge incompatible caches. Maintenance checks the toolchain at most once per
hour by default; the first maintenance call always checks for an update.

## Toolchain lifecycle

Before accepting builds, and periodically in a long-running builder, update the
configured toolchain. The caller decides whether a compiler change requires
regenerating and publishing shared rustdoc files:

```rust,no_run
# use anyhow::Result;
# use docs_rs_rustwide::BuildEnvironment;
# use rustwide::Toolchain;
# use std::path::Path;
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
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

Environment initialization installs a missing toolchain. For distribution
toolchains it also installs the docs.rs default targets and attempts to install
`llvm-tools-preview`, `rustc-dev`, and `rustfmt`; unavailable components produce
warnings and do not prevent initialization. Non-default targets
left by individual crate builds are removed before a distribution toolchain is
updated. CI toolchains are installed and treated as changed on every update.

A durable service should additionally compare `rustc_version()` with the version
of the essential files it last published. That ensures generation and
publication are retried after a failure even when the installed compiler no
longer changes on the next update check. The published version should only be
recorded after publication succeeds.

## Host resources and compiler metrics

After fetching the release, when `FetchedRelease::run` starts, the environment
verifies that the host's available memory can satisfy the effective sandbox
limit. Callers can archive sources before this check. This check
is enabled by default and can be disabled when the caller intentionally wants
the sandbox or host runtime to enforce the limit:

```rust,no_run
# use anyhow::Result;
# use docs_rs_rustwide::BuildEnvironment;
# use std::path::Path;
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
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
# use docs_rs_rustwide::BuildEnvironment;
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
    .compiler_metrics_collection_path("./compiler-metrics")
    .build()?;
let krate = Crate::crates_io("serde", "1.0.219");

let result = environment.release(&krate).run(|build| build.build_docs())?;
# let _ = result;
# Ok(())
# }
```

For HTML builds, the library passes rustdoc's unstable metrics directory flag
when the metrics directory is available. `build_docs` collects metrics after
each HTML attempt into `TargetBuildResult::compiler_metrics`, a separate
`StepResult<Vec<PathBuf>>`. Collection failures do not invalidate HTML. Custom
builds using `build_documentation` should call `collect_compiler_metrics` afterward
if they need the metrics copied to the configured destination.

## Complete release build

The usual entry point builds coverage, rustdoc JSON, and HTML documentation for
the default target and, if it produces library documentation, additional targets
selected by the crate's metadata. Set `.include_default_targets(true)` to also
use the docs.rs default target list when metadata does not specify targets:

```rust,no_run
use anyhow::Result;
use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
use rustwide::Crate;
use std::path::Path;

fn main() -> Result<()> {
    docs_rs_rustwide::logging::init(true);
    let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
        .sandbox_image(SandboxImageSource::LocalOrRemote(
            "ghcr.io/rust-lang/crates-build-env/linux".into(),
        ))
        .build()?;

    let krate = Crate::crates_io("serde", "1.0.219");
    let build = environment
        .release(&krate)
        .run(|build| build.build_docs())?;

    // Includes fetch, sandbox setup/teardown, and cleanup; excludes environment
    // setup.
    println!("build duration: {:?}", build.duration());
    println!("sandbox statistics: {:#?}", build.statistics());
    let release = build.into_inner();
    for target in release.targets {
        println!(
            "{}: documentation succeeded: {}",
            target.target,
            target.documentation_succeeded()
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
# use docs_rs_rustwide::{BuildEnvironment, SandboxImageSource};
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
# let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
#     .sandbox_image(SandboxImageSource::LocalOrRemote("ghcr.io/rust-lang/crates-build-env/linux".into()))
#     .build()?;
# let krate = Crate::crates_io("serde", "1.0.219");
let selected = environment.release(&krate).run(|build| {
    let target = build.metadata_targets().default_target.to_owned();
    let json = build.build_rustdoc_json(&target);
    let documentation = build.build_documentation(&target);
    Ok((json, documentation))
})?;
# let _ = selected;
# Ok(())
# }
```

See [`examples/custom_build.rs`](examples/custom_build.rs).

Individual step methods return `StepResult<T>` with a `duration`, captured `log`,
and `outcome: Result<T, BuildStepError>`. Errors identify the failing phase:
`Prepare`, `Command`, or `Output`. The full release build aborts on default-target
preparation failures and applies the default-target lockfile retry to HTML command
failures. Additional-target preparation failures remain in their step results,
allowing successful default-target documentation to be published.
Coverage, JSON, and metrics output failures are nonfatal. Metrics collection has
its own step result and cannot invalidate successful HTML documentation.

Call `step.into_result()?` when a custom build requires a step to succeed. This
propagates a `FailedStep` containing the error, duration, and log. Higher-level
methods preserve it through `anyhow::Error` for `downcast_ref::<FailedStep>()`.
Its duration covers only the failing step, not the full target or release.

## Archiving sources before a build

`ReleaseContext::fetch` exposes an intermediate phase for callers that need to
archive sources before metadata parsing or sandbox preparation:

```rust,no_run
# use anyhow::Result;
# use docs_rs_rustwide::BuildEnvironment;
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# docs_rs_rustwide::logging::init(true);
# let mut environment = BuildEnvironment::builder(Path::new("./rustwide-workspace")).build()?;
let krate = Crate::crates_io("serde", "1.0.219");
let fetched = environment
    .release(&krate)
    .fetch()?;

fetched.copy_source_to("./source-archive-input")?;

let result = fetched.run(|build| build.build_docs())?;

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

## Exclusive workspace access

`BuildEnvironment` holds an exclusive filesystem lock from initialization until
it is dropped, including across workspace refreshes and toolchain maintenance.
The CLI and production builder fail with a workspace-in-use error if another
environment owns the directory. Use a separate workspace for concurrent builders.
Tests can opt into waiting with `.wait_for_workspace_lock(true)`.

Keep the environment alive until artifacts have been consumed or copied out.
Starting another release or refreshing the workspace purges previous build
directories, including generated artifacts. Copy files that must survive first.
Do not remove `.docsrs-workspace.lock` while an environment is running. The file
may remain after exit; ownership is released automatically when its handle closes.
