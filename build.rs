use anyhow::{Context as _, Error, Result};
use git2::Repository;
use std::{env, fs::File, io::Write, path::Path};

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
    let dest_path = out_dir.join("git_version");

    let mut file = File::create(&dest_path)?;
    write!(file, "({} {})", git_hash, build_date)?;

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

fn compile_sass_file(
    out_dir: &Path,
    name: &str,
    target: &str,
    include_paths: &[String],
) -> Result<()> {
    use sass_rs::{Context, Options, OutputStyle};

    const STYLE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/templates/style");

    let include_paths = {
        let mut paths = vec![STYLE_DIR.to_owned()];
        paths.extend_from_slice(include_paths);
        paths
    };

    for path in &include_paths {
        for entry in walkdir::WalkDir::new(path) {
            println!("cargo:rerun-if-changed={}", entry?.path().display());
        }
    }

    let mut context =
        Context::new_file(format!("{}/{}.scss", STYLE_DIR, name)).map_err(Error::msg)?;
    context.set_options(Options {
        output_style: OutputStyle::Compressed,
        include_paths,
        ..Default::default()
    });

    let css = context.compile().map_err(Error::msg)?;
    let dest_path = out_dir.join(format!("{}.css", target));
    let mut file = File::create(&dest_path)?;
    file.write_all(css.as_bytes())?;

    Ok(())
}

fn compile_sass(out_dir: &Path) -> Result<()> {
    // Compile base.scss -> style.css
    compile_sass_file(out_dir, "base", "style", &[])?;

    // Compile rustdoc.scss -> rustdoc.css
    compile_sass_file(out_dir, "rustdoc", "rustdoc", &[])?;
    compile_sass_file(out_dir, "rustdoc-2021-12-05", "rustdoc-2021-12-05", &[])?;

    // Compile vendored.scss -> vendored.css
    compile_sass_file(
        out_dir,
        "vendored",
        "vendored",
        &[concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/pure-css/css").to_owned()],
    )?;

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
