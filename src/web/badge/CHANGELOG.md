# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `Badge::new()` will now return an error if the status or subject is empty
- `Badge::new()` now caches `DejaVuSans.ttf` the first time it is loaded,
  improving latency at a small memory cost.

### Fixed

- `rusttype` has been upgraded to `0.9`,
  removing the dependency on the deprecated crate `stb_truetype`
- `base64` has been upgraded to `0.12`
