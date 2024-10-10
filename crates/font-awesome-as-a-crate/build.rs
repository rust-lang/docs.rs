use std::{
    collections::HashMap,
    env,
    fmt::Write as FmtWrite,
    fs::{read_dir, File},
    io::{Read, Write},
    path::Path,
};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(font_awesome_out_dir)");
    println!("cargo:rustc-cfg=font_awesome_out_dir");
    write_fontawesome_sprite();
}

fn capitalize_first_letter(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().chain(c).collect(),
    }
}

fn write_fontawesome_sprite() {
    let mut types = HashMap::new();
    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("fontawesome.rs");
    let mut dest_file = File::create(dest_path).unwrap();
    dest_file
        .write_all(b"const fn fontawesome_svg(dir:&str,file:&str)->&'static str{match(dir.as_bytes(),file.as_bytes()){")
        .expect("fontawesome fn write");
    for (dirname, trait_name) in &[
        ("brands", "Brands"),
        ("regular", "Regular"),
        ("solid", "Solid"),
    ] {
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
            let filename = filename.replace(".svg", "");
            dest_file
                .write_all(
                    format!(r####"(b"{dirname}",b"{filename}")=>r#"{data}"#,"####).as_bytes(),
                )
                .expect("write fontawesome file");
            types
                .entry(filename)
                .or_insert_with(|| (data.clone(), Vec::with_capacity(3)))
                .1
                .push(trait_name);
        }
    }
    dest_file
        .write_all(b"_=>\"\"}} pub mod icons { use super::{IconStr, Regular, Brands, Solid};")
        .expect("fontawesome fn write");

    for (icon, (data, kinds)) in types {
        let mut type_name = "Icon".to_string();
        type_name.extend(icon.split('-').map(capitalize_first_letter));
        let kinds = kinds.iter().fold(String::new(), |mut acc, k| {
            let _ = writeln!(acc, "impl {k} for {type_name} {{}}");
            acc
        });
        dest_file
            .write_all(
                format!(
                    "\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct {type_name};
impl IconStr for {type_name} {{
    fn icon_name(&self) -> &'static str {{ r#\"{icon}\"# }}
    fn icon_svg(&self) -> &'static str {{ r#\"{data}\"# }}
}}
{kinds}"
                )
                .as_bytes(),
            )
            .expect("write fontawesome file types");
    }

    dest_file.write_all(b"}").expect("fontawesome fn write");
}
