# Build Server

The build servers:

- read releases from the build queue,
- use [`rustwide`](https://docs.rs/rustwide/latest/rustwide/) to run
  `cargo doc`, isolating each build in a Docker container for security, and
- package the documentation into a ZIP file and upload it to S3.

The code lives
[in the `docs_rs_builder` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/docs_rs_builder).

## Build environment

Docs.rs / `rustwide` are internally using the
[`crates-build-env` docker images](https://github.com/rust-lang/crates-build-env)
as the build environment for the crate. If you're missing a system dependency,
you can add it there.

Also see the [docs.rs build info page](https://docs.rs/about/builds).
