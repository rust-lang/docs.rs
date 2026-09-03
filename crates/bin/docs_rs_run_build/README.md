# docs_rs_run_build

`docs_rs_run_build` runs a crate through the same Cargo, rustdoc, rustwide, and
sandbox configuration used by docs.rs. It is intended for crate authors who want
to catch documentation build failures locally or in CI before publishing.

## Requirements

- A Linux host
- A running Docker daemon accessible to the current user
- Rust and Cargo installed through rustup

The command itself must run on the host. Running it inside a container while
controlling sibling Docker containers is not currently supported. On macOS or
Windows, run it in a Linux VM or a Linux CI job.

## Installation

Once the crate is published, install the locked release with:

```console
cargo install docs_rs_run_build --locked
```

When developing docs.rs itself, install the workspace copy with:

```console
cargo install --path crates/bin/docs_rs_run_build --locked
```

## Building a crate

From a package directory:

```console
docs_rs_run_build
```

Or pass the package directory explicitly:

```console
docs_rs_run_build path/to/package
```

Before starting the sandbox, the command runs
`cargo package --allow-dirty
--no-verify` and extracts the resulting crate
archive. The build therefore uses the files and normalized manifest that would
be published, rather than the whole source checkout. Packaging errors are
treated as build failures.

Dirty working trees are accepted so the command can test uncommitted changes.
Cargo's `include` and `exclude` rules still apply.

## Cargo workspaces

When the provided path is a workspace root that also contains a package, that
root package is built by default. For a virtual workspace, select a member
explicitly:

```console
docs_rs_run_build --package my-crate
```

The package argument accepts the same package specification syntax as
`cargo
package --package`. A virtual workspace without `--package` is rejected
rather than implicitly selecting a member.

You can also point directly at a member directory:

```console
docs_rs_run_build crates/my-crate
```

## GitHub Actions

A minimal workflow job looks like this:

```yaml
name: docs.rs build

on:
  pull_request:
  push:

permissions:
  contents: read

jobs:
  docs-rs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install docs.rs build runner
        run: cargo install docs_rs_run_build --locked
      - name: Build documentation as docs.rs
        run: docs_rs_run_build --package my-crate
```

Omit `--package` for a repository whose root manifest is the package being
built.

## Sandbox image

The normal docs.rs build image is used by default. For faster testing with the
smaller image used by the build library's integration tests, pass:

```console
docs_rs_run_build --small-image
```

A custom image and its resolution policy can be selected with `--image` and
`--image-source`.

## Build behavior and exit status

By default, the command fails when setup, packaging, the default-target HTML
build, or production of the crate's library documentation fails. Failures in
rustdoc JSON, documentation coverage, or additional targets are reported but do
not change the exit status, matching how docs.rs treats auxiliary output.

Use `--strict` to make any auxiliary or additional-target failure fatal:

```console
docs_rs_run_build --strict
```

Cargo and rustdoc output is streamed to standard output. A final summary shows
the result for every target and the paths of generated HTML and rustdoc JSON
artifacts.

## Workspace and generated files

Rustwide state, caches, and generated artifacts are stored in
`target/docsrs-build` below the path passed to the command. Override that
location when necessary:

```console
docs_rs_run_build --workspace /tmp/docsrs-workspace
```

The exact artifact paths are printed in the build summary. A later invocation
may clean old release build directories, so copy artifacts needed after the CI
job before starting another build with the same workspace.

## Configuration

The default toolchain is nightly and the default sandbox limits match docs.rs.
Documentation targets are selected through the crate's docs.rs metadata;
toolchains, images, CPU and memory limits, networking, timeouts, and failure
policy can be adjusted through command-line options.

Run the following for the authoritative list of options and defaults:

```console
docs_rs_run_build --help
```
