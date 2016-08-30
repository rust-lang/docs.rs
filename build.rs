
extern crate time;
extern crate sass_rs;

use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Write;
use std::process::Command;

fn main() {
    let git_hash = match Command::new("git")
        .args(&["log", "--pretty=format:%h", "-n", "1"])
        .output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(_) => "???????".to_string(),
    };
    let build_date = time::strftime("%Y-%m-%d", &time::now_utc()).unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("git_version");
    let mut file = File::create(&dest_path).unwrap();
    write!(file, "({} {})", git_hash, build_date).unwrap();

    // compile style.scss
    compile_sass();
}



fn compile_sass() {
    use sass_rs::sass_context::SassFileContext;

    let mut file_context = SassFileContext::new(concat!(env!("CARGO_MANIFEST_DIR"),
                                                        "/templates/style.scss"));
    let css = file_context.compile().unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("style.css");
    let mut file = File::create(&dest_path).unwrap();
    file.write_all(css.as_bytes()).unwrap();
}
