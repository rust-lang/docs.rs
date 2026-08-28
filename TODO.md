# TODO

- Rustdoc JSON metadata:
  - reading format_version from the generated JSON;
  - the new crate returns only the JSON path.

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
