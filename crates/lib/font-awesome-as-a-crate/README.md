# [Font Awesome Free](https://fontawesome.com/download) SVG files as a crate

This is not officially supported by Fonticons, Inc.
If you have problems, [contact us](https://github.com/rust-lang/docs.rs), not them.

## Updating Font Awesome

The crate vendors the Font Awesome Free desktop SVG distribution. Its `build.rs`
generates Rust types and embeds the icons at compile time.

To update it:

1. Download the new Font Awesome Free desktop SVG distribution from the
   [Font Awesome website](https://fontawesome.com/download).
2. Replace the `fontawesome-free-*-desktop` directory and retain the
   distribution's license file.
3. Update the vendored distribution path in `build.rs`.
4. Build and test the crate from the docs.rs workspace root:

   ```console
   $ cargo test --package font-awesome-as-a-crate
   ```

Publishing is handled by `.github/workflows/publish.yml`, which runs
`cargo publish --package font-awesome-as-a-crate` when a matching version tag is
pushed.

The older docs.rs webfont integration and its `$fa-font-path` setting are no
longer used.
