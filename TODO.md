# TODO

- [ ] clean up toolchain management logic from AI draft

- [ ] can the workspace do the toolchain refresh, and workspace reinit on its
      own? so a user of the library doesn't have to remember that?

- `builder.reinitialize_workspace_if_interval_passed`
- `builder.update_toolchain_and_add_essential_files`

## Library gaps that affect migration

1. Source access and source archiving

The old builder archives crate sources before entering the rustwide build
callback, even if metadata parsing or preparation later fails. See
crates/bin/docs_rs_builder/src/docbuilder/ rustwide_builder.rs:642.

The new crate hides its Workspace, and ReleaseBuild does not expose
host_source_dir(). The binary therefore cannot currently:

- call Crate::copy_source_to;
- archive sources before metadata parsing;
- inspect the source tree for examples/;
- pass the source directory to finish_release.

I would add something like:

impl BuildEnvironment { pub fn copy_crate_source_to( &self, krate: &Crate,
destination: impl AsRef<Path>, ) -> Result<()>; }

impl ReleaseBuild<'_, '_> { pub fn source_dir(&self) -> &Path; }

The first method preserves the old “source archive exists even when preparation
fails” behavior. The second supports metadata and examples inspection during the
callback.

2. Essential-files publication acknowledgement

The current lifecycle marks the toolchain prepared inside update_toolchain()
before essential files are built or published. See
crates/lib/docs_rs_build/src/workspace.rs:233.

If essential-file generation or storage upload fails:

- toolchain_prepared remains true;
- the next dist-toolchain update sees the same rustc version;
- update_toolchain_and_build_essential_files() returns None;
- essential files are not retried.

The old builder records the rustc version in the database only after
successfully publishing essential files.

For production, either:

- remove process-local publication tracking and let the binary compare
  environment.rustc_version() with the database version; or
- add an explicit acknowledgement lifecycle, such as
  mark_essential_files_published().

I prefer the first option. The database is already the durable authority.

3. Essential-files limits

The old builder obtains limits for the dummy empty-library crate from the
database. The new build_essential_files() always uses
BuildEnvironment::default_limits.

If per-crate overrides for empty-library need preserving, add:

environment .release(&essential_files_crate) .limits(limits)

internally through a build_essential_files_with_limits(limits) method, or let
the binary create the essential-files release directly.

## Configuration migration needed

The old configuration collapses these into one field:

DOCSRS_LOCAL_DOCKER_IMAGE .or(DOCSRS_DOCKER_IMAGE)

The new API needs to know whether the image is:

- Local;
- Remote;
- LocalOrRemote.

To refresh a remote tag periodically, the binary configuration should preserve
which environment variable supplied the image. Otherwise it cannot reliably
choose SandboxImageSource::Remote.

The remaining settings map directly:

- workspace path;
- inside-Docker flag;
- workspace refresh interval;
- CPU quota or core range;
- Docker runtime;
- default-target inclusion;
- host memory validation;
- compiler-metrics destination;
- default limits.

## Binary orchestration that intentionally remains

The following old-builder work should stay in docs_rs_builder:

- blacklist checks;
- database crate/release/build initialization;
- per-crate limit lookup;
- source archive upload;
- documentation archive layout and upload;
- JSON compression and versioned/latest uploads;
- build-log upload;
- recording coverage;
- repository lookup and statistics;
- crates.io API data;
- detecting examples;
- documentation-size calculation;
- OpenTelemetry counters;
- preserving a previous successful release after a failed rebuild;
- cleaning legacy storage paths;
- queue locking and retry policy.

The new results already provide nearly everything needed for this:

- Cargo and docs.rs metadata;
- target results and logs;
- documentation paths;
- has_docs;
- coverage;
- rustdoc JSON path and lazy format version;
- compiler-metrics paths;
- rustwide sandbox statistics;
- BuildEnvironment::rustc_version();
- BUILDER_VERSION.

## Result interpretation during migration

The binary must deliberately distinguish:

target.documentation.successful()

The HTML command succeeded.

target.successful()

The command succeeded and its output directory exists.

target.has_docs(library_name)

The output contains documentation for the crate’s actual library target.

For exact old database behavior, BuildStatus should be based on the HTML command
result, while release has_docs and successful-target publication should use the
stronger filesystem checks.

## Artifact lifetime

Returned documentation and JSON paths point into the rustwide build directory.
They remain available after release().run(...), but only until another release
purges build directories.

The migrated builder must therefore copy/compress/upload all returned artifacts
before starting the next release. That matches the existing serial queue, but
should be documented explicitly.

## Cleanup before landing

Two smaller items remain:

- The README references selected_targets(), while the actual method is
  metadata_targets().
- The absolute local rustwide [patch.crates-io] dependency is appropriate for
  development but must not land in the final production configuration.

So the main implementation blocker is source access. The main correctness
blocker is durable acknowledgement of essential-file publication. Once those are
resolved, the old build_package_inner() can be replaced with a relatively thin
database/storage adapter around release().run(|build| build.build_docs()).
