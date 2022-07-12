use anyhow::{Context as _, Error, Result};
use git2::Repository;
use std::{env, path::Path};

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

    // TODO: are these right?
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    Ok(())
}

fn get_git_hash() -> Result<Option<String>> {
    match Repository::open(env::current_dir()?) {
        Ok(repo) => {
            let head = repo.head()?;

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
        &grass::Options::default().style(grass::OutputStyle::Compressed),
    )
    .map_err(|e| Error::msg(e.to_string()))?;

    std::fs::write(dest, css)?;

    Ok(())
}

fn compile_sass(out_dir: &Path) -> Result<()> {
    const STYLE_DIR: &str = "templates/style";

    for entry in walkdir::WalkDir::new(STYLE_DIR) {
        let entry = entry?;
        println!(
            "cargo:rerun-if-changed={}",
            entry
                .path()
                .to_str()
                .with_context(|| format!("{} is a non-utf-8 path", entry.path().display()))?
        );
        let file_name = entry.file_name().to_str().unwrap();
        if entry.metadata()?.is_file() && !file_name.starts_with('_') {
            let dest = out_dir
                .join(entry.path().strip_prefix(STYLE_DIR)?)
                .with_extension("css");
            compile_sass_file(entry.path(), &dest).with_context(|| {
                format!("compiling {} to {}", entry.path().display(), dest.display())
            })?;
        }
    }

    // Compile vendored.css
    println!("cargo:rerun-if-changed=vendor/pure-css/css/pure-min.css");
    let pure = std::fs::read_to_string("vendor/pure-css/css/pure-min.css")?;
    println!("cargo:rerun-if-changed=vendor/pure-css/css/grids-responsive-min.css");
    let grids = std::fs::read_to_string("vendor/pure-css/css/grids-responsive-min.css")?;
    let vendored = pure + &grids;
    std::fs::write(out_dir.join("vendored").with_extension("css"), vendored)?;

    Ok(())
}

fn write_known_targets(out_dir: &Path) -> Result<()> {
    use std::io::BufRead;

    let targets: Vec<String> = std::process::Command::new("rustc")
        .args(&["--print", "target-list"])
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
