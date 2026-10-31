`shrust-0.0.4.index.zst` is the Zstandard-compressed SQLite archive index for
the source archive of `shrust` version `0.0.4`. It contains 80,827 file entries,
including the crate's vendored `gh-pages` documentation, and is used by the
`folder_contents` benchmarks.

It was generated from the supplied `0.0.4.zip` using
`docs_rs_storage::archive_index::create`, the same index builder used in
production. The ZIP is omitted because these benchmarks only query the index;
they do not read file contents. Keep this fixture in the production index
format, including file ranges and compression values.

Compress with `zstd -9 shrust-0.0.4.index`. The benchmarks decompress it into a
temporary file once during setup, before any measurements.
