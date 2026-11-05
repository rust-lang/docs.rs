`shrust-0.0.4.index.zst` is the Zstandard-compressed SQLite archive index for
the source archive of `shrust` version `0.0.4`. It contains 80,827 file entries,
including the crate's vendored `gh-pages` documentation, and is used by the
`archive_index_read` benchmarks.

It was generated from the supplied `0.0.4.zip` using
`docs_rs_storage::archive_index::create`, the same index builder used in
production. The ZIP is omitted because these benchmarks only query the index;
they do not read file contents. Keep this fixture in the production index
format, including file ranges and compression values.

Compress with `zstd -9 shrust-0.0.4.index`. The benchmarks decompress it into a
temporary file once during setup, before any measurements.

Run the read benchmarks with:

```sh
cargo bench -p docs_rs_storage --features testing --bench archive_index_read
```

They cover point lookups, counting and streaming all entries on a reused
connection, directory listings on reused connections, connection initialization,
and fresh connections for concurrent lookups and root directory listings.
Concurrency levels are 1, 4, 16, 64, 128, and 256; reported times for these
cases are for the whole batch, with throughput measured in requests. The index
and filesystem caches are warm; S3 downloads, cache repair, and archive payload
reads are excluded.

These concurrent cases submit synchronized batches to measure burst handling,
including queueing at the SQLite read concurrency limit. They do not model a
fixed request arrival rate such as 1,000 requests per second.
