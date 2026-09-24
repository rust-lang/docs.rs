# docs_rs_build

`docs_rs_build` runs a crate through the same Cargo, rustdoc, rustwide, and
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

Temporarily install from Git while crates.io publication and the internal
dependency releases are being prepared. Replace `<commit>` with a commit that
contains `docs_rs_build`; pinning it keeps the installation reproducible.

```console
cargo install --git https://github.com/rust-lang/docs.rs --rev "<commit>" docs_rs_build --locked
```

When developing docs.rs itself, install the workspace copy with:

```console
cargo install --path crates/bin/docs_rs_build --locked
```

## Building a crate

From a package directory:

```console
docs_rs_build
```

Or pass the package directory explicitly:

```console
docs_rs_build path/to/package
```

Before starting the sandbox, the command runs
`cargo package --allow-dirty --no-verify` and extracts the resulting crate
archive. The build uses the files and normalized manifest prepared for
publication. The CLI adds an empty `[workspace]` table to the extracted manifest
to isolate it from any surrounding workspace; the original manifest is
unchanged. Packaging errors are treated as build failures.

Dirty working trees are accepted so the command can test uncommitted changes.
Cargo's `include` and `exclude` rules still apply.

The CLI does not add docs.rs's default target list. Targets configured in
package metadata still apply.

## Cargo workspaces

Without `--package`, package selection follows `cargo package`. At a workspace
root that contains a package, `workspace.default-members` can select another
member or multiple members; otherwise Cargo selects the root package. The CLI
requires exactly one generated crate archive and fails if multiple are produced.

Use `--package` to explicitly select one package. This is required for virtual
workspaces, even when they configure `default-members`:

```console
docs_rs_build --package my-crate
```

The package argument accepts the same package specification syntax as
`cargo package --package`. A virtual workspace without `--package` is rejected
rather than implicitly selecting a member.

You can also point directly at a member directory:

```console
docs_rs_build crates/my-crate
```

## GitHub Actions

A workflow template using the temporary Git installation looks like this.
Replace `<commit>` with the revision to install:

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
    env:
      DOCSRS_IMAGE: ghcr.io/rust-lang/crates-build-env/linux
      DOCSRS_IMAGE_ARCHIVE: ${{ runner.temp }}/docs-rs-image.tar.zst
    steps:
      - uses: actions/checkout@v7
      - name: Cache the rustwide workspace
        uses: actions/cache@v5
        with:
          path: target/docsrs-build
          key: docs-rs-build-v1-${{ runner.os }}-${{ runner.arch }}
      - name: Restore the docs.rs Docker image archive
        id: docsrs-image-cache
        uses: actions/cache@v5
        with:
          path: ${{ env.DOCSRS_IMAGE_ARCHIVE }}
          # Bump this version when the mutable `linux` image should be refreshed.
          key: docs-rs-image-v1-${{ runner.os }}-${{ runner.arch }}
      - name: Load the cached docs.rs Docker image
        if: steps.docsrs-image-cache.outputs.cache-hit == 'true'
        run: zstd --decompress --stdout "$DOCSRS_IMAGE_ARCHIVE" | docker load
      - name: Pull and archive the docs.rs Docker image
        if: steps.docsrs-image-cache.outputs.cache-hit != 'true'
        run: |
          docker pull "$DOCSRS_IMAGE"
          docker save "$DOCSRS_IMAGE" |
            zstd --threads=0 -3 --output "$DOCSRS_IMAGE_ARCHIVE"
      - name: Install docs.rs build runner
        # Temporary until docs_rs_build and its dependencies are published to crates.io.
        run: cargo install --git https://github.com/rust-lang/docs.rs --rev "<commit>" docs_rs_build --locked
      - name: Build documentation as docs.rs
        run: docs_rs_build --package my-crate
```

Omit `--package` only when Cargo's default selection produces the single package
you intend to build and the manifest is not a virtual workspace. The cached
`target/docsrs-build` directory preserves rustwide's rustup installation,
toolchains, Cargo cache, and other workspace state between CI runs. The cache
version only needs to be changed if the workspace layout becomes incompatible.

The Docker image is stored as a compressed `docker save` archive because
GitHub-hosted runners start each job with a fresh Docker daemon. On a cache hit,
`docker load` makes the image available before `docs_rs_build` initializes its
workspace. GitHub Actions caches are immutable, while the `linux` image tag is
mutable, so increment `docs-rs-image-v1` whenever the workflow should fetch a
new image. A pinned image tag or digest can instead be included in the cache
key.

## Sandbox image

The normal docs.rs build image is used by default. It contains a broad set of
native libraries so that docs.rs can build crates with system dependencies, but
that compatibility makes the initial download large. As of 2026-09-03, the
current amd64 image has approximately 3.4 GB of compressed layers and takes more
space after extraction.

For faster testing with the smaller image used by the build library's
integration tests, pass:

```console
docs_rs_build --small-image
```

The corresponding amd64 micro image is approximately 259 MB compressed. It is a
good choice when the crate does not rely on native packages available only in
the full image; otherwise, use the default image for the closest reproduction of
docs.rs.

A custom image and its resolution policy can be selected with `--image` and
`--image-source`. The source policy also applies to the default image and
`--small-image`: `local` requires a cached image, `remote` pulls it, and the
default `local-or-remote` pulls only when the image is missing locally.

### Caching the image in CI

The example workflow uses `actions/cache` to preserve a compressed image archive
and loads it into the fresh Docker daemon at the start of the job. This can
avoid repeatedly pulling the image, but the full image remains a large cache
entry. Restoring it can transfer roughly the same amount of data as an image
pull while adding `docker save`/`docker load` overhead and consuming the
repository's cache allowance. Measure both approaches for your workload.

For frequent builds, prefer one of these approaches:

- Use `--small-image` when the crate does not need the full image's native
  dependencies.
- Run multiple documentation checks in the same job, where Docker reuses the
  already-pulled layers.
- Use a self-hosted runner with a persistent Docker daemon. The default
  `local-or-remote` image policy then reuses its local image; use
  `--image-source remote` when the job must refresh the image tag.

## Build behavior and exit status

By default, the command fails when setup, packaging, the default-target HTML
build, or production of the crate's library documentation fails. JSON and
coverage failures, and additional-target failures, are reported but do not
change the default exit status. This includes preparation, command, and
output-processing failures for those steps.

A default-target HTML preparation or command failure retries once with a
regenerated lockfile when one exists. This reruns coverage, JSON, and HTML. If
lockfile regeneration fails, its error and captured log are reported alongside
the original failed HTML build. Release fetching and initial Cargo metadata
failures return early. Additional targets are built only when the default target
produces library documentation. The CLI does not have the production builder's
queue reattempt mechanism.

Use `--strict` to make JSON, coverage, or additional-target failures affect the
exit status:

```console
docs_rs_build --strict
```

Cargo and rustdoc build output is streamed live to stderr. When the release
completes, a table on stdout shows HTML, JSON, and coverage status/duration for
each target, totals, full build duration, and sandbox peak memory. Displayed
durations are rounded to milliseconds. Errors and captured logs for failed
steps, including lockfile-regeneration failures, are written to stderr. Coverage
is shown as skipped for additional targets. Setup and release-fetch errors
return early with an error instead of the summary table. Packaging output is
streamed live to stderr too.

## Workspace and generated files

Rustwide state, caches, and generated artifacts are stored in
`target/docsrs-build` below the path passed to the command. Override that
location when necessary:

```console
docs_rs_build --workspace /tmp/docsrs-workspace
```

HTML and JSON are moved into unique locations under
`builds/<name>-<version>-<hash>/tmp/<target>/` within the workspace. The exact
paths are printed in the build summary on stdout. Dropping build results or
exiting the command does not delete them. Rustwide's build-directory cleanup
removes them, so copy anything you need to retain before starting another build
with the same workspace. The workspace is locked for the lifetime of the build
environment; concurrent invocations must use different workspace directories.

## Configuration

Use `--experimental` to opt into proposed docs.rs build defaults. Currently this
denies `rustdoc::invalid_html_tags` and checks unknown lint names. The
experimental defaults may change between releases; without the flag they are not
enabled.

```console
docs_rs_build --experimental
```

To enforce individual lints, configure them in your crate, for example with
`#![deny(rustdoc::invalid_html_tags)]` in `src/lib.rs`. Experimental lint
defaults affect rustdoc, not rustc compilation of dependencies. By default, no
lint flags are added.

The default toolchain is nightly and the default sandbox limits match docs.rs.
Workspace initialization reuses installed Rustwide helper tools through fast
initialization. An installed distribution toolchain is checked for updates
unless `--no-update-toolchain` is set. A missing toolchain is always installed;
CI toolchains are not automatically updated by the CLI. Documentation targets
are selected through the crate's docs.rs metadata; toolchains, images, CPU and
memory limits, networking, timeouts, and failure policy can be adjusted through
command-line options.

Run the following for the authoritative list of options and defaults:

```console
docs_rs_build --help
```
