use anyhow::{Context as _, Error, Result};
use std::{collections::BTreeMap, env, fs::File, io::Write as _, path::Path};

mod tracked {
    use std::{
        collections::HashSet,
        io::{Error, Result},
        path::{Path, PathBuf},
        sync::{LazyLock, Mutex},
    };

    static SEEN: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

    pub(crate) fn track(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            let mut seen = SEEN.lock().unwrap();
            // TODO: Needs something like `HashSet::insert_owned` to check before cloning
            // https://github.com/rust-lang/rust/issues/60896
            if !seen.contains(path) {
                seen.insert(path.to_owned());
                let path = path.to_str().ok_or_else(|| {
                    Error::other(format!("{} is a non-utf-8 path", path.display()))
                })?;
                println!("cargo:rerun-if-changed={path}");
            }
        } else if let Some(parent) = path.parent() {
            // if the file doesn't exist, we need to notice if it begins existing
            track(parent)?;
        }
        Ok(())
    }

    pub(crate) fn track_recursive(path: impl AsRef<Path>) -> Result<()> {
        for entry in walkdir::WalkDir::new(path) {
            track(entry?.path())?;
        }
        Ok(())
    }

    pub(crate) fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
        let path = path.as_ref();
        track(path)?;
        std::fs::read(path)
    }

    pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        track(path)?;
        std::fs::read_to_string(path)
    }

    #[derive(Debug)]
    pub(crate) struct Fs;

    impl grass::Fs for Fs {
        fn is_dir(&self, path: &Path) -> bool {
            track(path).unwrap();
            path.is_dir()
        }
        fn is_file(&self, path: &Path) -> bool {
            track(path).unwrap();
            path.is_file()
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            read(path)
        }
    }
}

type ETagMap = BTreeMap<String, String>;

fn main() -> Result<()> {
    let out_dir = env::var("OUT_DIR").context("missing OUT_DIR")?;
    let out_dir = Path::new(&out_dir);

    let mut etag_map: ETagMap = ETagMap::new();

    compile_sass(out_dir, &mut etag_map)?;
    compile_syntax(out_dir).context("could not compile syntax files")?;
    calculate_static_etags(&mut etag_map)?;

    let mut etag_file = File::create(out_dir.join("static_etag_map.rs"))?;
    writeln!(etag_file, "pub const STATIC_ETAG_MAP: &[(&str, &str)] = &[")?;
    for (path, etag) in etag_map.iter() {
        // the debug repr of a `str` is also a valid escaped string literal in the code
        writeln!(etag_file, r#"    ({:?}, {:?}), "#, path, etag)?;
    }
    writeln!(etag_file, "];")?;

    etag_file.sync_all()?;

    // trigger recompilation when a new migration is added
    println!("cargo:rerun-if-changed=migrations");
    Ok(())
}

fn etag_from_path(path: impl AsRef<Path>) -> Result<String> {
    Ok(etag_from_content(std::fs::read(&path)?))
}

fn etag_from_content(content: impl AsRef<[u8]>) -> String {
    let digest = md5::compute(content);
    let md5_hex = format!("{:x}", digest);
    format!(r#""{md5_hex}""#)
}

fn compile_sass_file(src: &Path, dest: &Path) -> Result<()> {
    let css = grass::from_path(
        src.to_str()
            .context("source file path must be a utf-8 string")?,
        &grass::Options::default()
            .fs(&tracked::Fs)
            .style(grass::OutputStyle::Compressed),
    )
    .map_err(|e| Error::msg(e.to_string()))?;

    std::fs::write(dest, css)?;

    Ok(())
}

fn compile_sass(out_dir: &Path, etag_map: &mut ETagMap) -> Result<()> {
    const STYLE_DIR: &str = "templates/style";

    for entry in walkdir::WalkDir::new(STYLE_DIR) {
        let entry = entry?;
        if entry.metadata()?.is_dir() {
            tracked::track(entry.path())?;
        } else {
            let file_name = entry
                .file_name()
                .to_str()
                .context("file name must be a utf-8 string")?;
            if !file_name.starts_with('_') {
                let dest = out_dir.join(file_name).with_extension("css");
                compile_sass_file(entry.path(), &dest).with_context(|| {
                    format!("compiling {} to {}", entry.path().display(), dest.display())
                })?;

                let dest_str = dest.file_name().unwrap().to_str().unwrap().to_owned();
                etag_map.insert(dest_str, etag_from_path(&dest)?);
            }
        }
    }

    // Compile vendored.css
    let pure = tracked::read_to_string("vendor/pure-css/css/pure-min.css")?;
    let grids = tracked::read_to_string("vendor/pure-css/css/grids-responsive-min.css")?;
    let vendored = pure + &grids;
    std::fs::write(out_dir.join("vendored").with_extension("css"), &vendored)?;

    etag_map.insert(
        "vendored.css".to_owned(),
        etag_from_content(vendored.as_bytes()),
    );

    Ok(())
}

fn calculate_static_etags(etag_map: &mut ETagMap) -> Result<()> {
    const STATIC_DIRS: &[&str] = &["static", "vendor"];

    for static_dir in STATIC_DIRS {
        for entry in walkdir::WalkDir::new(static_dir) {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let partial_path = path.strip_prefix(static_dir).unwrap();
            let partial_path_str = partial_path.to_string_lossy().to_string();
            etag_map.insert(partial_path_str, etag_from_path(path)?);
        }
    }

    Ok(())
}

fn compile_syntax(out_dir: &Path) -> Result<()> {
    use syntect::{
        dumps::dump_to_uncompressed_file,
        parsing::{SyntaxDefinition, SyntaxSetBuilder},
    };

    fn tracked_add_from_folder(
        builder: &mut SyntaxSetBuilder,
        path: impl AsRef<Path>,
    ) -> Result<()> {
        // There's no easy way to know exactly which files matter, so just track everything in the
        // folder
        tracked::track_recursive(&path)?;
        builder.add_from_folder(path, true)?;
        Ok(())
    }

    let mut builder = SyntaxSetBuilder::new();
    builder.add_plain_text_syntax();

    tracked_add_from_folder(&mut builder, "assets/syntaxes/Packages/")?;

    // The TOML syntax already includes `Cargo.lock` in its alternative file extensions, but we
    // also want to support `Cargo.toml.orig` files.
    let mut toml = SyntaxDefinition::load_from_str(
        &tracked::read_to_string("assets/syntaxes/Extras/TOML/TOML.sublime-syntax")?,
        true,
        Some("TOML"),
    )?;
    toml.file_extensions.push("Cargo.toml.orig".into());
    builder.add(toml);

    tracked_add_from_folder(
        &mut builder,
        "assets/syntaxes/Extras/JavaScript (Babel).sublime-syntax",
    )?;

    dump_to_uncompressed_file(&builder.build(), out_dir.join("syntect.packdump"))?;

    Ok(())
}
