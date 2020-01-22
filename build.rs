
extern crate time;
extern crate sass_rs;
extern crate git2;
extern crate minifier;

use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Write;
use git2::Repository;


fn main() {
    write_git_version();
    compile_sass();
    minify_js();
}


fn write_git_version() {
    let git_hash = get_git_hash().unwrap_or("???????".to_owned());
    let build_date = time::strftime("%Y-%m-%d", &time::now_utc()).unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("git_version");
    let mut file = File::create(&dest_path).unwrap();
    write!(file, "({} {})", git_hash, build_date).unwrap();
}


fn get_git_hash() -> Option<String> {
    let repo = match Repository::open(env::current_dir().unwrap()) {
        Ok(repo) => repo,
        Err(_) => return None,
    };
    let head = repo.head().unwrap();
    head.target().map(|h| {
        let mut h = format!("{}", h);
        h.truncate(7);
        h
    })
}


fn compile_sass() {
    use sass_rs::Context;

    let mut file_context = Context::new_file(concat!(env!("CARGO_MANIFEST_DIR"),
                                                     "/templates/style.scss")).unwrap();
    let css = file_context.compile().unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("style.css");
    let mut file = File::create(&dest_path).unwrap();
    file.write_all(css.as_bytes()).unwrap();
}

fn minify_js() {
    use minifier::js::minify;
    use std::io::Read;
    let mut javascript = String::new();
    let source_path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/menu.js"));
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("menu.js");
    File::open(&source_path)
        .expect("open templates/menu.js")
        .read_to_string(&mut javascript)
        .expect("read templates/menu.js");
    let javascript = minify(&javascript);
    File::create(&dest_path)
        .expect("create target/.../menu.js")
        .write_all(javascript.as_bytes())
        .expect("write target/.../menu.js");
}
