use git2::Repository;
use std::{
    env,
    error::Error,
    fs::{self, File},
    io::Write,
    path::Path,
};

fn main() {
    // Set the host target
    println!(
        "cargo:rustc-env=CRATESFYI_HOST_TARGET={}",
        env::var("TARGET").unwrap(),
    );

    // Don't rerun anytime a single change is made
    println!("cargo:rerun-if-changed=templates/style/base.scss");
    println!("cargo:rerun-if-changed=templates/style/_rustdoc.scss");
    println!("cargo:rerun-if-changed=templates/style/_vars.scss");
    println!("cargo:rerun-if-changed=templates/style/_utils.scss");
    println!("cargo:rerun-if-changed=templates/style/_navbar.scss");
    println!("cargo:rerun-if-changed=templates/menu.js");
    println!("cargo:rerun-if-changed=templates/index.js");
    println!("cargo:rerun-if-changed=vendor/");
    // TODO: are these right?
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    write_git_version();
    if let Err(sass_err) = compile_sass() {
        panic!("Error compiling sass: {}", sass_err);
    }
    copy_js();
}

fn write_git_version() {
    let maybe_hash = get_git_hash();
    let git_hash = maybe_hash.as_deref().unwrap_or("???????");

    let build_date = time::strftime("%Y-%m-%d", &time::now_utc()).unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("git_version");

    let mut file = File::create(&dest_path).unwrap();
    write!(file, "({} {})", git_hash, build_date).unwrap();
}

fn get_git_hash() -> Option<String> {
    let repo = Repository::open(env::current_dir().unwrap()).ok()?;
    let head = repo.head().unwrap();

    head.target().map(|h| {
        let mut h = format!("{}", h);
        h.truncate(7);
        h
    })
}

fn compile_sass() -> Result<(), Box<dyn Error>> {
    use sass_rs::{Context, Options, OutputStyle};

    const STYLE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/templates/style");

    let mut context = Context::new_file(format!("{}/base.scss", STYLE_DIR))?;
    context.set_options(Options {
        output_style: OutputStyle::Compressed,
        include_paths: vec![
            STYLE_DIR.to_owned(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/fontawesome/scss").to_owned(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/pure-css/css").to_owned(),
        ],
        ..Default::default()
    });

    let css = context.compile()?;
    let dest_path = Path::new(&env::var("OUT_DIR")?).join("style.css");
    let mut file = File::create(&dest_path)?;
    file.write_all(css.as_bytes())?;

    Ok(())
}

fn copy_js() {
    ["menu.js", "index.js"].iter().for_each(|path| {
        let source_path =
            Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join(format!("templates/{}", path));
        let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join(path);
        fs::copy(&source_path, &dest_path).expect("Copy JavaScript file to target");
    });
}
