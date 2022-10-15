use anyhow::{Context as _, Error, Result};
use git2::Repository;
use std::{env, path::Path};

mod tracked {
    use once_cell::sync::Lazy;
    use std::{
        collections::HashSet,
        io::{Error, ErrorKind, Result},
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static SEEN: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(|| Mutex::new(HashSet::new()));

    pub(crate) fn track(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            let mut seen = SEEN.lock().unwrap();
            // TODO: Needs something like `HashSet::insert_owned` to check before cloning
            // https://github.com/rust-lang/rust/issues/60896
            if !seen.contains(path) {
                seen.insert(path.to_owned());
                let path = path.to_str().ok_or_else(|| {
                    Error::new(
                        ErrorKind::Other,
                        format!("{} is a non-utf-8 path", path.display()),
                    )
                })?;
                println!("cargo:rerun-if-changed={path}");
            }
        } else if let Some(parent) = path.parent() {
            // if the file doesn't exist, we need to notice if it begins existing
            track(parent)?;
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

fn main() -> Result<()> {
    let out_dir = env::var("OUT_DIR").context("missing OUT_DIR")?;
    let out_dir = Path::new(&out_dir);
    write_git_version(out_dir)?;
    compile_sass(out_dir)?;
    write_known_targets(out_dir)?;
    Ok(())
}

fn write_git_version(out_dir: &Path) -> Result<()> {
    let maybe_hash = get_git_hash()?;
    let git_hash = maybe_hash.as_deref().unwrap_or("???????");

    let build_date = time::OffsetDateTime::now_utc().date();

    std::fs::write(
        out_dir.join("git_version"),
        format!("({} {})", git_hash, build_date),
    )?;

    Ok(())
}

fn get_git_hash() -> Result<Option<String>> {
    match Repository::open(env::current_dir()?) {
        Ok(repo) => {
            let head = repo.head()?;

            // TODO: are these right?
            tracked::track(".git/HEAD")?;
            tracked::track(".git/index")?;

            Ok(head.target().map(|h| {
                let mut h = format!("{}", h);
                h.truncate(7);
                h
            }))
        }
        Err(err) => {
            eprintln!("failed to get git repo: {err}");
            Ok(None)
        }
    }
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

fn compile_sass(out_dir: &Path) -> Result<()> {
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
                let dest = out_dir
                    .join(entry.path().strip_prefix(STYLE_DIR)?)
                    .with_extension("css");
                compile_sass_file(entry.path(), &dest).with_context(|| {
                    format!("compiling {} to {}", entry.path().display(), dest.display())
                })?;
            }
        }
    }

    // Compile vendored.css
    let pure = tracked::read_to_string("vendor/pure-css/css/pure-min.css")?;
    let grids = tracked::read_to_string("vendor/pure-css/css/grids-responsive-min.css")?;
    let vendored = pure + &grids;
    std::fs::write(out_dir.join("vendored").with_extension("css"), vendored)?;

    Ok(())
}

fn write_known_targets(out_dir: &Path) -> Result<()> {
    use std::io::BufRead;

    let targets: Vec<String> = std::process::Command::new("rustc")
        .args(["--print", "target-list"])
        .output()?
        .stdout
        .lines()
        .filter(|s| s.as_ref().map_or(true, |s| !s.is_empty()))
        .collect::<std::io::Result<_>>()?;

    string_cache_codegen::AtomType::new("target::TargetAtom", "target_atom!")
        .atoms(&targets)
        .write_to_file(&out_dir.join("target_atom.rs"))?;

    Ok(())
}
