use std::{
    env,
    fs::{read_dir, File},
    io::{Read, Write},
    path::Path,
};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(font_awesome_out_dir)");
    println!("cargo:rustc-cfg=font_awesome_out_dir");
    write_fontawesome_sprite();
}

fn write_fontawesome_sprite() {
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("fontawesome.rs");
    let mut dest_file = File::create(dest_path).unwrap();
    dest_file
        .write_all(b"const fn fontawesome_svg(dir:&str,file:&str)->&'static str{match(dir.as_bytes(),file.as_bytes()){")
        .expect("fontawesome fn write");
    for dirname in &["brands", "regular", "solid"] {
        let dir = read_dir(Path::new("fontawesome-free-6.2.0-desktop/svgs").join(dirname)).unwrap();
        let mut data = String::new();
        for file in dir {
            let file = file.expect("fontawesome directory access");
            let filename = file
                .file_name()
                .into_string()
                .expect("fontawesome filenames are unicode");
            let mut file = File::open(file.path()).expect("fontawesome file access");
            data.clear();
            file.read_to_string(&mut data)
                .expect("fontawesome file read");
            // if this assert goes off, add more hashes here and in the format! below
            assert!(!data.contains("###"), "file {filename} breaks raw string");
            dest_file
                .write_all(
                    format!(
                        r####"(b"{dirname}",b"{filename}")=>r#"{data}"#,"####,
                        data = data,
                        dirname = dirname,
                        filename = filename.replace(".svg", ""),
                    )
                    .as_bytes(),
                )
                .expect("write fontawesome file");
        }
    }
    dest_file
        .write_all(b"_=>\"\"}}")
        .expect("fontawesome fn write");
}
