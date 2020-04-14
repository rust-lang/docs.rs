use git2::Repository;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

fn main() {
    // Set the host target
    println!(
        "cargo:rustc-env=CRATESFYI_HOST_TARGET={}",
        env::var("TARGET").unwrap()
    );

    write_git_version();
    compile_sass();
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

fn compile_sass() {
    use sass_rs::Context;

    let mut file_context =
        Context::new_file(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/style.scss")).unwrap();
    let css = file_context.compile().unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("style.css");
    let mut file = File::create(&dest_path).unwrap();
    file.write_all(css.as_bytes()).unwrap();
}

fn copy_js() {
    ["menu.js", "index.js"].iter().for_each(|path| {
        let source_path =
            Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join(format!("templates/{}", path));
        let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join(path);
        fs::copy(&source_path, &dest_path).expect("Copy JavaScript file to target");
    });
}
