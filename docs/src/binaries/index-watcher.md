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
$ just watcher queue set-last-seen-reference <GIT_REF>
```

Omit the reference, or pass `--head`, to reset it to the index's current HEAD.

## Scheduled database synchronization

Run `just watcher database synchronize` to repair inconsistencies. Full
synchronization reads the index at the watcher's last-seen commit, or the
fetched HEAD when no last-seen reference has been set. Synchronization does not
initialize or advance the reference.

Synchronization, registry polling, and manual reference changes share a database
lock. The watcher can remain running: it waits while synchronization runs, then
processes changes after that reference. Builders continue running. Concurrent
synchronizations also wait for each other. Use `--dry-run` to inspect repairs;
it takes the same lock and reads the same commit.

Single-crate synchronization uses the same Git snapshot selection: the watcher's
last-seen commit, or fetched HEAD when no reference exists. It takes the same
lock while checking and repairing the crate, without advancing the reference.

## Watching a subset of crates

Set `DOCSRS_ONLY_EXISTING_CRATES=true` to keep a smaller database in
environments such as staging. The setting defaults to `false`, which watches all
crates.

With this setting enabled:

- Live registry ingestion only processes changes for crates with a row in the
  `crates` table. An empty database therefore queues no builds.
- Full database synchronization only considers crates with stored releases or
  queued builds. Crate rows without either are ignored.
- Synchronizing a single named crate still queues its missing releases, even if
  the crate is not yet in the database.

To add a crate, manually queue a build, build or import a release, or
synchronize that crate explicitly:

```console
$ just watcher database synchronize <CRATE_NAME>
```

A manually queued build runs normally. Once the builder creates the crate's row
in `crates`, live ingestion starts tracking future changes for it. The queue
entry alone does not enable live ingestion. A `crates` row is sufficient even
before any releases are stored.

The registry cursor advances past ignored changes, so adding a crate does not
replay them. Explicit synchronization queues missing releases from its history.
Automatic rebuilds already select existing releases from the database.
