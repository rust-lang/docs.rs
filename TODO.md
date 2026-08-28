# TODO

The core Cargo/rustdoc build path is mostly present, but several old-builder
capabilities are still absent.

Most importantly, the old docs_rs_builder does not use the new crate yet. It has
no docs_rs_build dependency and still executes its original implementation
independently.

Missing service-independent or potentially reusable pieces:

- Toolchain lifecycle:
  - installing/updating the configured toolchain;
  - installing required components such as rust-src;
  - maintaining the default target set;
  - detecting whether the toolchain changed;
  - retrying failed toolchain downloads;
  - purging caches after an update.

- CI toolchain handling:
  - the old builder synthesizes a rustc version for CI toolchains because normal
    version detection does not work;
  - the new resource_suffix() always invokes rustc --version.

- Workspace lifecycle:
  - periodic workspace reinitialization;
  - public cache purging with purge_all_caches().
  - Per-release purge_all_build_dirs() is already present.

- Host resource validation:
  - the old builder checks available host memory before starting a build;
  - the new crate applies the sandbox limit but does not verify that the host
    can satisfy it.

- Compiler metrics collection:
  - injecting -Zmetrics-dir;
  - creating and copying the metrics directory;
  - the collect_metrics build option.

- Rustdoc JSON metadata:
  - reading format_version from the generated JSON;
  - the new crate returns only the JSON path.

- Documentation existence semantics:
  - the old builder verifies that output directories actually exist before
    marking additional targets successful;
  - the new TargetBuildResult::successful() only checks whether the HTML command
    returned an error.
  - The default-target flow does perform a stronger library-directory check
    before building additional targets.

- Essential-files semantics:
  - the new crate builds and returns the static-files output directory;
  - it does not fail the outer operation when that StepResult is unsuccessful;
  - it does not normalize the static.files subdirectory. Storage/uploading
    should remain outside, but identifying the actual static-files directory
    could belong in the library.

- Result information:
  - rustc version;
  - docs.rs version;
  - an explicit has_docs result;
  - JSON format version;
  - optional compiler metrics paths.

Intentionally service-specific pieces that should probably stay in the binary:

- Database build/release initialization and completion.
- Build queue summaries and reattempt decisions.
- Per-crate limit lookup from the database.
- Blacklist lookup.
- Source archiving.
- Documentation and log uploads.
- JSON compression and uploads.
- Storage cleanup.
- Crates.io API data.
- Repository records and statistics.
- Metrics counters.
- Preserving an old successful release when a rebuild fails.
- Detecting examples for database metadata.

Already represented in the new crate:

- Rustwide workspace and sandbox setup.
- CPU, memory, timeout, networking, and log-size limits.
- Crate fetching and cache cleanup.
- Metadata-derived target selection.
- Target-count limiting.
- Lazy target installation.
- Build-std dependency preparation.
- Correct Cargo and rustdoc arguments.
- Coverage, rustdoc JSON, and HTML builds in one sandbox.
- Captured per-step logs and errors.
- Default-target lockfile regeneration and retry.
- Proc-macro output-directory handling.
- Cargo metadata loading.
- Shared rustdoc static-file generation.

The highest-priority next step is wiring the old builder to use the new crate.
That integration will expose which missing pieces genuinely belong in the
library versus which are only needed by the docs.rs service.
