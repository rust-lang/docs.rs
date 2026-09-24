# Updating Vendored Frontend Assets

Docs.rs keeps some frontend dependencies in the repository so builds do not
depend on downloading assets at runtime.

## Font Awesome

Font Awesome Free SVGs are packaged by the `font-awesome-as-a-crate` workspace
crate. The vendored distribution, license, and generator live under
`crates/lib/font-awesome-as-a-crate/`.

See the crate's
[README](https://github.com/rust-lang/docs.rs/tree/main/crates/lib/font-awesome-as-a-crate#updating-font-awesome)
for the update and release procedure.

## Pure CSS

The minified Pure CSS files and their license live under
`crates/bin/docs_rs_web/vendor/pure-css/`. The web crate's build script reads
`pure-min.css` and `grids-responsive-min.css` from that directory and combines
them with the site's generated CSS.

When updating Pure CSS, replace those minified files from an official
[Pure CSS release](https://purecss.io/start/), update the vendored license when
needed, and run:

```console
$ cargo test --package docs_rs_web
$ just prepare-gui-tests run-gui-tests
```

For the container integration path, set both CLI modes to `docker` and run
`just prepare-gui-tests run-gui-tests-e2e` instead.
