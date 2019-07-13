
extern crate time;
extern crate sass_rs;

use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Write;
use std::process::Command;

fn main() {
    write_git_version();
    compile_sass();
}


fn write_git_version() {
    let git_hash = get_git_hash().unwrap_or("???????".to_owned());
    let build_date = time::strftime("%Y-%m-%d", &time::now_utc()).unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("git_version");
    let mut file = File::create(&dest_path).unwrap();
    write!(file, "({} {})", git_hash, build_date).unwrap();
}


fn get_git_hash() -> Option<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--short=7")
        .arg("HEAD")
        .output()
        .ok()?;
    Some(String::from_utf8(output.stdout).ok()?)
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
