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

Once the crate is published, install the locked release with:

```console
cargo install docs_rs_build --locked
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

A workflow template for use once `docs_rs_build` is published looks like this:

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
        run: cargo install docs_rs_build --locked
      - name: Build documentation as docs.rs
        run: docs_rs_build --package my-crate
```

Omit `--package` for a repository whose root manifest is the package being
built. The cached `target/docsrs-build` directory preserves rustwide's rustup
installation, toolchains, Cargo cache, and other workspace state between CI
runs. The cache version only needs to be changed if the workspace layout becomes
incompatible.

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
build, or production of the crate's library documentation fails. Preparation
failures during any default-target step also abort the build. JSON and coverage
command/output failures, and additional-target failures including preparation,
are reported but do not change the default exit status.

A default-target HTML command failure retries once with a regenerated lockfile
when one exists. This reruns coverage, JSON, and HTML. Lockfile regeneration or
dependency-fetch failure aborts the build. Additional targets are built only
when the default target produces library documentation. The CLI does not have
the production builder's queue reattempt mechanism.

Use `--strict` to make JSON, coverage, or additional-target failures affect the
exit status:

```console
docs_rs_build --strict
```

Cargo and rustdoc build output is streamed live. When the release completes, a
table shows HTML, JSON, and coverage status/duration for each target, totals,
full build duration, and sandbox peak memory. Failed steps include their captured
logs. Setup and fatal preparation errors return early with an error instead of
the summary table. Packaging output is printed after `cargo package` finishes.

## Workspace and generated files

Rustwide state, caches, and generated artifacts are stored in
`target/docsrs-build` below the path passed to the command. Override that
location when necessary:

```console
docs_rs_build --workspace /tmp/docsrs-workspace
```

The exact artifact paths are printed in the build summary. A later invocation
purges old release build directories, so copy artifacts needed after the CI job
before starting another build with the same workspace. The workspace is locked
for the lifetime of the build environment; concurrent invocations must use
different workspace directories.

## Configuration

The default toolchain is nightly and the default sandbox limits match docs.rs.
An installed distribution toolchain is checked for updates unless
`--no-update-toolchain` is set. A missing toolchain is always installed; CI
toolchains are not automatically updated by the CLI.
Documentation targets are selected through the crate's docs.rs metadata;
toolchains, images, CPU and memory limits, networking, timeouts, and failure
policy can be adjusted through command-line options.

Run the following for the authoritative list of options and defaults:

```console
docs_rs_build --help
```
