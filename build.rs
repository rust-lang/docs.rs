
extern crate time;

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
        Err(_)     => "???????".to_string()
    };
    let build_date = time::strftime("%Y-%m-%d", &time::now_utc()).unwrap();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("git_version");
    let mut file = File::create(&dest_path).unwrap();
    write!(file, "\" ({} {})\"", git_hash, build_date).unwrap();
}
