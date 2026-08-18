# Index Watcher

The index watcher is a small process that manages a clone of the
[`crates.io-index` repository](https://github.com/rust-lang/crates.io-index).

The watcher updates the clone once a minute and uses
[`crates-index-diff`](https://docs.rs/crates-index-diff/latest/crates_index_diff/)
to determine the changes.

Depending on the change, we:

- add the release to the build queue,
- update the release's yanked status, or
- delete the crate or release entirely from our storage.

The code lives
[in the `docs_rs_watcher` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/docs_rs_watcher).
