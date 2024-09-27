use proc_macro2::Literal;
use quote::{format_ident, quote};
use std::{collections::HashMap, env, fs, path::Path};

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
    let mut match_arms = vec![];
    let mut types = HashMap::new();

    for (dirname, trait_name) in &[
        ("brands", "Brands"),
        ("regular", "Regular"),
        ("solid", "Solid"),
    ] {
        let dir =
            fs::read_dir(Path::new("fontawesome-free-6.2.0-desktop/svgs").join(dirname)).unwrap();

        for file in dir {
            let file = file.expect("fontawesome directory access");
            let data = fs::read_to_string(file.path()).expect("fontawesome file read");

            let filename = file
                .file_name()
                .into_string()
                .expect("fontawesome filenames are unicode")
                .replace(".svg", "");

            let dirname_literal = Literal::byte_string(dirname.as_bytes());
            let filename_literal = Literal::byte_string(filename.as_bytes());
            match_arms.push(quote! {
                (#dirname_literal, #filename_literal) => #data,
            });

            types
                .entry(filename)
                .or_insert_with(|| (data.clone(), Vec::with_capacity(3)))
                .1
                .push(trait_name);
        }
    }

    let mut types_output = vec![];

    for (icon, (data, kinds)) in types {
        let mut type_name = "Icon".to_string();
        type_name.extend(icon.split('-').map(capitalize_first_letter));
        let type_name = format_ident!("{}", type_name);

        let kind_impls: Vec<_> = kinds
            .iter()
            .map(|k| {
                let k = format_ident!("{}", k);
                quote! {
                    impl #k for #type_name {}
                }
            })
            .collect();

        types_output.push(quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct #type_name;

            impl IconStr for #type_name {
                fn icon_name(&self) -> &'static str { #icon }
                fn icon_str(&self) -> &'static str { #data }
            }

            #(#kind_impls)*
        });
    }

    let token_stream = quote! {
        const fn fontawesome_svg(dir: &str, file: &str) -> &'static str {
            // we are using byte literals to match because they can be evaluated in a
            // `const` context, and `str` cannot.
            match(dir.as_bytes(), file.as_bytes()) {
                #(#match_arms)*
                _=> ""
            }
        }

        pub mod icons {
            use super::{IconStr, Regular, Brands, Solid};

            #(#types_output)*
        }
    };

    let dest_path = Path::new(&env::var("OUT_DIR").unwrap()).join("fontawesome.rs");

    let output = prettyplease::unparse(&syn::parse2(token_stream).unwrap());

    fs::write(&dest_path, output.as_bytes()).expect("fontawesome output write");
}
