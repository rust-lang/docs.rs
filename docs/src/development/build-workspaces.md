# Build workspace packages

Use `docs_rs_build` to check a local package with the docs.rs build pipeline. It
requires a Linux host, Docker, and Rust installed through rustup, but no docs.rs
database.

From the docs.rs repository, select a package in another workspace:

```console
cargo run --locked -p docs_rs_build -- /path/to/workspace --package my_lib
```

Or point directly at the member directory:

```console
cargo run --locked -p docs_rs_build -- /path/to/workspace/my_lib
```

If the CLI is already installed, the equivalent command is:

```console
docs_rs_build /path/to/workspace --package my_lib
```

## Packaging and inherited workspace settings

The CLI runs `cargo package --allow-dirty --no-verify` automatically and
extracts its archive before building. Cargo normalizes the packaged manifest,
resolving inherited values such as `version.workspace = true`. The CLI also adds
an empty `[workspace]` table to the extracted manifest so Cargo does not
discover an unrelated parent workspace. The original manifest is unchanged.

For example, a workspace can define:

```toml
[workspace]
members = ["my_lib"]

[workspace.package]
version = "0.1.0"
```

And `my_lib/Cargo.toml` can inherit that version:

```toml
[package]
name = "my_lib"
version.workspace = true
```

With `my_lib/src/lib.rs` present, either command above packages and builds the
member without a separate manual packaging step. Cargo's packaging rules still
apply, including file inclusion and dependency requirements. Dirty working trees
are accepted so uncommitted changes can be tested.

## Selecting a package

Virtual workspaces require `--package`, even if they define `default-members`.
For a workspace root that is also a package, selection without `--package`
follows `cargo package`, including `workspace.default-members`. The CLI requires
exactly one generated archive; use `--package` to make the selection explicit.

The CLI reports results and retained artifact paths; it does not publish the
local build into a docs.rs development instance. For building registry releases
or importing published documentation into that instance, see [README.md].

{{#include ../links.md}}
