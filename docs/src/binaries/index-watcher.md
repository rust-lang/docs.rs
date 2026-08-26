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

## Starting position in local development

On its first run, the watcher stores the current HEAD of the registry index as
its last-seen reference. It therefore watches new changes rather than queuing
every release already present in the index.

To start from a particular Git reference, set it before starting the watcher:

```console
$ just cli-queue-reset-last-seen-ref <GIT_REF>
```

Omit the reference, or pass `--head`, to reset it to the index's current HEAD.
