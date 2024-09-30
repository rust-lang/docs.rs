use anyhow::{Context as _, Error, Result};
use font_awesome_as_a_crate as f_a;
use std::{
    env,
    path::{Path, PathBuf},
};

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

fn main() -> Result<()> {
    let out_dir = env::var("OUT_DIR").context("missing OUT_DIR")?;
    let out_dir = Path::new(&out_dir);
    write_git_version(out_dir)?;
    compile_sass(out_dir)?;
    write_known_targets(out_dir)?;
    compile_syntax(out_dir).context("could not compile syntax files")?;

    println!("cargo::rustc-check-cfg=cfg(icons_out_dir)");
    println!("cargo:rustc-cfg=icons_out_dir");

    let package_dir = env::var("CARGO_MANIFEST_DIR").context("missing CARGO_MANIFEST_DIR")?;
    let package_dir = Path::new(&package_dir);
    generate_css_icons(package_dir.join("static/icons.css"), out_dir)?;

    // trigger recompilation when a new migration is added
    println!("cargo:rerun-if-changed=migrations");
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().chain(c).collect(),
    }
}

fn render_icon(
    icon_name: &str,
    icon_str: &str,
    type_name: String,
    code_output: &mut String,
    css_output: &mut String,
    icon_kind: &str,
) {
    let css_class = format!("f-a_{icon_name}_{icon_kind}");
    css_output.push_str(&format!(
        "\
.{css_class} {{
    --svg_{icon_name}_{icon_kind}: url('data:image/svg+xml,{icon_str}');
    -webkit-mask: var(--svg_{icon_name}_{icon_kind}) no-repeat center;
    mask: var(--svg_{icon_name}_{icon_kind}) no-repeat center;
}}
",
    ));
    let type_name = format!("{type_name}{}", capitalize(icon_kind));
    code_output.push_str(&format!(
        r#"#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct {type_name};
impl {type_name} {{
    pub fn render(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String> {{
        render({css_class:?}, fw, spin, extra)
    }}
}}
"#,
    ));
}

fn generate_css_icons(css_path: PathBuf, out_dir: &Path) -> Result<()> {
    let mut code_output = r#"pub(crate) mod icons {
    fn render(
        css_class: &str,
        fw: bool,
        spin: bool,
        extra: &str,
    ) -> rinja::filters::Safe<String> {
        let mut classes = vec!["fa-svg"];
        if fw {
            classes.push("fa-svg-fw");
        }
        if spin {
            classes.push("fa-svg-spin");
        }
        if !extra.is_empty() {
            classes.push(extra);
        }
        let icon = format!(
            "<span class=\"{css_class} {class}\" aria-hidden=\"true\"></span>",
            class = classes.join(" "),
        );

        rinja::filters::Safe(icon)
    }"#
    .to_string();
    let mut css_output = r#".svg-clipboard {
    /* This icon is copied from crates.io */
    --svg-clipboard: url('data:image/svg+xml,<svg width="24" height="25" viewBox="0 0 24 25" fill="currentColor" xmlns="http://www.w3.org/2000/svg" aria-label="Copy to clipboard"><path d="M18 20h2v3c0 1-1 2-2 2H2c-.998 0-2-1-2-2V5c0-.911.755-1.667 1.667-1.667h5A3.323 3.323 0 0110 0a3.323 3.323 0 013.333 3.333h5C19.245 3.333 20 4.09 20 5v8.333h-2V9H2v14h16v-3zM3 7h14c0-.911-.793-1.667-1.75-1.667H13.5c-.957 0-1.75-.755-1.75-1.666C11.75 2.755 10.957 2 10 2s-1.75.755-1.75 1.667c0 .911-.793 1.666-1.75 1.666H4.75C3.793 5.333 3 6.09 3 7z"/><path d="M4 19h6v2H4zM12 11H4v2h8zM4 17h4v-2H4zM15 15v-3l-4.5 4.5L15 21v-3l8.027-.032L23 15z"/></svg>');
    -webkit-mask: var(--svg-clipboard) no-repeat center;
    mask: var(--svg-clipboard) no-repeat center;
}"#.to_string();

    let brands: &[&dyn f_a::Brands] = &[
        &f_a::icons::IconFonticons,
        &f_a::icons::IconRust,
        &f_a::icons::IconMarkdown,
        &f_a::icons::IconGitAlt,
    ];
    let regular: &[&dyn f_a::Regular] = &[
        &f_a::icons::IconFileLines,
        &f_a::icons::IconFolderOpen,
        &f_a::icons::IconFile,
        &f_a::icons::IconStar,
    ];
    let solid: &[&dyn f_a::Solid] = &[
        &f_a::icons::IconCircleInfo,
        &f_a::icons::IconGears,
        &f_a::icons::IconTable,
        &f_a::icons::IconRoad,
        &f_a::icons::IconDownload,
        &f_a::icons::IconCubes,
        &f_a::icons::IconSquareRss,
        &f_a::icons::IconFileLines,
        &f_a::icons::IconCheck,
        &f_a::icons::IconTriangleExclamation,
        &f_a::icons::IconGear,
        &f_a::icons::IconX,
        &f_a::icons::IconHouse,
        &f_a::icons::IconCodeBranch,
        &f_a::icons::IconStar,
        &f_a::icons::IconCircleExclamation,
        &f_a::icons::IconCube,
        &f_a::icons::IconChevronLeft,
        &f_a::icons::IconChevronRight,
        &f_a::icons::IconFolderOpen,
        &f_a::icons::IconLock,
        &f_a::icons::IconFlag,
        &f_a::icons::IconBook,
        &f_a::icons::IconMagnifyingGlass,
        &f_a::icons::IconLeaf,
        &f_a::icons::IconChartLine,
        &f_a::icons::IconList,
        &f_a::icons::IconUser,
        &f_a::icons::IconTrash,
        &f_a::icons::IconArrowLeft,
        &f_a::icons::IconArrowRight,
        &f_a::icons::IconLink,
        &f_a::icons::IconScaleUnbalancedFlip,
        &f_a::icons::IconSpinner,
    ];

    for icon in brands {
        render_icon(
            icon.icon_name(),
            icon.icon_str(),
            format!("{icon:?}"),
            &mut code_output,
            &mut css_output,
            "brands",
        );
    }
    for icon in regular {
        render_icon(
            icon.icon_name(),
            icon.icon_str(),
            format!("{icon:?}"),
            &mut code_output,
            &mut css_output,
            "regular",
        );
    }
    for icon in solid {
        render_icon(
            icon.icon_name(),
            icon.icon_str(),
            format!("{icon:?}"),
            &mut code_output,
            &mut css_output,
            "solid",
        );
    }

    std::fs::write(&css_path, css_output).map_err(|error| {
        Error::msg(format!(
            "Failed to write into `{}`: {error:?}",
            css_path.display()
        ))
    })?;

    code_output.push('}');
    let icons_file = out_dir.join("icons.rs");
    std::fs::write(&icons_file, code_output).map_err(|error| {
        Error::msg(format!(
            "Failed to write `{}`: {error:?}",
            icons_file.display()
        ))
    })?;
    Ok(())
}

fn write_git_version(out_dir: &Path) -> Result<()> {
    let maybe_hash = get_git_hash()?;
    let git_hash = maybe_hash.as_deref().unwrap_or("???????");

    let build_date = time::OffsetDateTime::now_utc().date();

    std::fs::write(
        out_dir.join("git_version"),
        format!("({git_hash} {build_date})"),
    )?;

    Ok(())
}

fn get_git_hash() -> Result<Option<String>> {
    match gix::open_opts(env::current_dir()?, gix::open::Options::isolated()) {
        Ok(repo) => {
            let head_id = repo.head()?.id();

            // TODO: are these right?
            tracked::track(".git/HEAD")?;
            tracked::track(".git/index")?;

            Ok(head_id.map(|h| format!("{}", h.shorten_or_id())))
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
