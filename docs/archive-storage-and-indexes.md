# Archive Storage and Indexes

This document explains how docs.rs stores rustdoc and source files in archive
storage, and how single-file requests are served efficiently without downloading
full archives.

## Why this exists

- We store many files per crate version (HTML, CSS, JS, source files, etc.).
- Downloading full archives on every file request would be expensive.
- We need fast random access to one file at a time.

## Storage model

For each crate version, docs.rs stores:

- one archive for rustdoc output;
- one archive for source files.

Both archives are ZIP files.

## Archive format assumptions

ZIP compression is per-file, not whole-archive.

- Each file entry has its own compressed payload.
- This allows serving one file by reading only the corresponding byte range.

This differs from `.tar.gz`:

- `tar` concatenates files into one stream;
- `gzip` then compresses that full stream as a whole;
- random access to one file is much harder without scanning/decompressing much
  more data.

## Index model

For each archive, docs.rs generates an index stored as an SQLite database.

The index is conceptually similar to a ZIP central directory, but represented in
a queryable format.

Each row maps a logical file path to the location of its compressed payload in
the archive:

- filename/path in archive;
- byte range start (`from`), inclusive;
- byte range end (`to`), inclusive;
- compression algorithm used for that file entry.

The range points to the compressed payload bytes of the ZIP entry (starting at
ZIP `data_start`), not to metadata headers.

We store the index file next to the ZIP on S3.

## Request flow (source or rustdoc file)

When a request asks for a single file:

1. Resolve which archive and index correspond to the crate/version.
2. Check whether the index is already present in local cache.
3. If missing, download the index file and cache it locally.
4. Query the SQLite index for the requested filename/path.
5. If no row exists, return "not found".
6. If found, read `from`/`to`/compression information.
7. Issue an HTTP Range request to S3 for only `[from, to]` from the remote ZIP
   file.
8. Decompress the returned byte range using the per-entry compression algorithm
   recorded in the index.
9. Return/use the decompressed file bytes as response content.

### Important details

- `archive_storage` is a per-release flag. If disabled, docs.rs serves files
  from legacy per-file objects instead of archive+index lookups.
- The local cache key includes `latest_build_id` (effectively
  `...zip.<build_id>.index`) so rebuilt releases naturally use a fresh local
  cache entry.
- Index cache population is concurrency-safe:
  - optimistic read first (no lock),
  - then per-index lock for repair/download,
  - temp-file download + atomic rename to publish.
- If index lookup/decompression fails (for example stale offsets causing
  decompression errors), docs.rs purges local cached index files and retries.

## Key properties

- Efficient network usage: fetches only bytes for the requested file.
- Efficient CPU usage: decompresses only one ZIP entry.
- Good cache behavior: index files can be cached locally and reused across many
  requests.
- better manageability or our S3 bucket, especially around rebuilds or deletions
  of crates or releases.

## Compression layers

There are two different compression layers involved:

- ZIP entry compression (inside rustdoc/source archives): this is what the
  archive index stores per file and what is used to decompress range responses.
  At the time of writing, archive index creation supports bzip2 ZIP entries.
- Object storage compression (for regular blob uploads): this is separate and
  represented by blob `Content-Encoding`/storage metadata.

For archive file serving, the important algorithm is the ZIP entry compression
from the index.

## Archive downloads

- these archives can also [be downloaded](https://docs.rs/about/download) to be
  used for offline docs.
- This is also why we used a more widely supported algorithm (Bz2) instead of
  zstd inside zip, which would theoretically also have been possible.

## Handler behavior and fallbacks

- Rustdoc/source handlers use the same archive lookup primitives.
- On missing rustdoc paths, handlers may try path fallbacks (for example
  appending `/index.html`) before returning 404.
- For target-specific misses, handlers can redirect to target fallback/search
  routes instead of always returning a hard 404.

## Notes for maintainers

- Keep index schema and archive writer in sync.
- Any change in byte-offset computation must preserve correct range boundaries.
- If index lookup fails unexpectedly, prefer rebuilding/downloading index rather
  than falling back to full archive downloads.
