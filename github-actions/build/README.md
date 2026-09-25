# docs.rs build action

A composite GitHub Action that runs the docs.rs build with nearly 100% parity
with our production systems.

It uses:

- `cargo package --allow-dirty --no-verify`, so this checks the files that would
  be published.
- the [docs.rs sandbox image][linux-image] and
- the same sandbox CPU / memory / disk limits, and
- the crate's [docs.rs metadata settings](https://docs.rs/about/metadata).

## Why test your crate with this action?

A successful local `cargo doc` build can still fail after publishing to docs.rs.
Adding this action to your crate's pull-request CI catches differences in the
published package, [documentation settings][docsrs-metadata], and build
environment before you release a version with broken hosted documentation.

It helps catch common failures described in the
[docs.rs build documentation](https://docs.rs/about/builds):

- **[docs.rs metadata][docsrs-metadata]:** The build uses
  [`[package.metadata.docs.rs]`][docsrs-metadata], including selected features,
  disabled default features, targets, and rustdoc options. This exercises
  configurations that your normal tests or `cargo doc` command might never
  build.
- **Missing system dependencies:** A native library or executable installed on
  your development machine might be absent from the docs.rs image. Building in
  that image reveals those dependencies; use the default
  [full image][linux-image] for the closest match to the libraries available on
  docs.rs.
- [**Read-only filesystem:**](https://docs.rs/about/builds#read-only-directories)
  The sandbox mounts the crate's source directory read-only. Build scripts that
  generate files in the source tree can fail here even when they work locally;
  generated build files should go in `OUT_DIR`.
- **Out-of-memory failures:** The sandbox applies docs.rs's default memory
  limit, helping expose documentation builds that succeed on a larger
  development machine but exceed the hosted build's budget. The CLI reports peak
  sandbox memory usage alongside build results.

The build also starts from a packaged crate, which can reveal files accidentally
excluded from publication. This is a useful rehearsal for publishing, though
nightly toolchain changes and crate-specific resource limits on docs.rs can
still affect the eventual hosted build.

## Alternative: cargo-docs-rs

[`cargo-docs-rs`](https://github.com/syphar/cargo-docs-rs) is a lighter-weight
alternative for local development or CI. It runs `cargo rustdoc` with
docs.rs-style options derived from
[`[package.metadata.docs.rs]`][docsrs-metadata], making it useful for checking
feature selection and documentation flags without downloading the sandbox image.

It runs in your existing environment, so it does not reproduce the docs.rs
image's system dependencies, read-only source mount, or sandbox memory limit.
Use this action when you also want to test those environment constraints.

## Usage

This action lives at `github-actions/build` in the docs.rs repository. In the
beta-phase, we're referencing using `main`. We'll do version numbers when we
fully release the tool.

```yaml
name: test docs.rs documentation build
on: [push, pull_request]
permissions:
  contents: read

jobs:
  docs-rs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: rust-lang/docs.rs/github-actions/build@main
        with:
          package: my-crate
          strict: "true"
```

The caller checks out the crate before invoking the action. Use a Linux host
with Bash, rustup, Cargo, and a running Docker daemon accessible to the runner
user. GitHub-hosted Ubuntu runners provide these prerequisites. Run directly on
the host: a job-level `container:` or Docker action controlling sibling
containers is not supported by `docs_rs_build`.

The action installs stable Rust for compiling the runner without changing the
caller's default toolchain. Documentation uses the CLI's default nightly
toolchain.

The `docs_rs_build` CLI is installed from the same repository snapshot as the
action using `GITHUB_ACTION_PATH`. Selecting `@main`, a tag, or a commit
therefore selects both the action and runner together, without a second Git
checkout or a separate runner revision input.

## Inputs

| Input          | Default | Meaning                                                                                                                           |
| -------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `path`         | `.`     | Crate or workspace directory relative to the checkout root.                                                                       |
| `package`      | empty   | Cargo package specification; required for virtual workspaces. If omitted, Cargo must select exactly one package.                  |
| `small-image`  | `false` | Use the smaller [linux-micro image][linux-micro-image] instead of the full [linux image][linux-image]. Accepts `true` or `false`. |
| `strict`       | `false` | Also fail for JSON, coverage, or additional-target failures. Accepts `true` or `false`.                                           |
| `experimental` | `false` | Enable experimental docs.rs build defaults. Accepts `true` or `false`.                                                            |

Default failure behavior matches the runner: setup, packaging, default-target
HTML, and missing library documentation fail the action. `strict: 'true'`
additionally makes auxiliary build failures fatal.

Set `experimental: "true"` to pass `--experimental` to the runner and enable
proposed docs.rs defaults, currently denying `rustdoc::invalid_html_tags` and
unknown lint names. These defaults may change between releases. This is separate
from `strict`, which only changes how build-step failures affect the result.

```yaml
- uses: rust-lang/docs.rs/github-actions/build@main
  with:
    experimental: "true"
```

To select a workspace member, run from the workspace root (the default
`path: .`) and specify its package name:

```yaml
- uses: rust-lang/docs.rs/github-actions/build@main
  with:
    package: my-crate
```

For faster image downloads, when the smaller image provides the native
dependencies your crate needs:

```yaml
- uses: rust-lang/docs.rs/github-actions/build@main
  with:
    small-image: "true"
```

The [full image][linux-image] provides closer parity with docs.rs. The
[micro image][linux-micro-image] has fewer native libraries.

## Artifacts

The `workspace` output gives the absolute path containing rustwide state and
generated documentation. The runner prints artifact locations in its summary;
the action does not upload documentation artifacts automatically.

Each invocation builds one package. Use separate matrix jobs to check multiple
crates concurrently. Host toolchain installation and Docker image downloads
require network access.

## References

- [docs_rs_build documentation](../../crates/bin/docs_rs_build/README.md)
- [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache)
- [GitHub composite action metadata](https://docs.github.com/en/actions/reference/workflows-and-actions/metadata-syntax#runs-for-composite-actions)

[linux-image]: https://github.com/rust-lang/crates-build-env/tree/master/linux
[linux-micro-image]: https://github.com/rust-lang/crates-build-env/tree/master/linux-micro
[docsrs-metadata]: https://docs.rs/about/metadata
