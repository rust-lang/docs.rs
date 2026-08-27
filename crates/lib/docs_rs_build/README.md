# docs_rs_build

`docs_rs_build` contains the service-independent part of the docs.rs build
pipeline. It configures rustwide, applies the docs.rs sandbox limits and Cargo
arguments, reads the crate's docs.rs metadata, and runs all build steps for a
release in one sandbox.

The crate does not store build results in the docs.rs database or copy artifacts
to docs.rs storage. A caller can decide what to do with the returned paths,
logs, coverage, and sandbox statistics.

## Complete release build

The usual entry point builds coverage, rustdoc JSON, and HTML documentation for
the default target and the additional targets selected by the crate's metadata:

```rust,no_run
use anyhow::Result;
use docs_rs_build::{BuildEnvironment, resolve_sandbox_image};
use rustwide::Crate;
use std::path::Path;

fn main() -> Result<()> {
    let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
        .sandbox_image(resolve_sandbox_image("docsrs/build-env:latest")?)
        .build()?;

    let krate = Crate::crates_io("serde", "1.0.219");
    let build = environment
        .release(&krate)
        .run(|build| build.build_targets())?;

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
# use docs_rs_build::{BuildEnvironment, resolve_sandbox_image};
# use rustwide::Crate;
# use std::path::Path;
# fn main() -> Result<()> {
# let environment = BuildEnvironment::builder(Path::new("./rustwide-workspace"))
#     .sandbox_image(resolve_sandbox_image("docsrs/build-env:latest")?)
#     .build()?;
# let krate = Crate::crates_io("serde", "1.0.219");
let selected = environment.release(&krate).run(|build| {
    let target = build.targets().default_target.to_owned();
    let json = build.build_rustdoc_json(&target);
    let documentation = build.build_documentation(&target);
    Ok((json, documentation))
})?;
# let _ = selected;
# Ok(())
# }
```

See [`examples/custom_build.rs`](examples/custom_build.rs).

`running_inside_docker(true)` is only needed when the calling program itself is
inside a container, for example a container action using the host Docker socket.
Leave it at its default (`false`) when invoking the program directly on a host.
