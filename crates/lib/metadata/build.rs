fn main() {
    // Set the host target
    println!(
        "cargo:rustc-env=DOCSRS_METADATA_HOST_TARGET={}",
        std::env::var("TARGET").unwrap(),
    );
    // This only needs to be rerun if the TARGET changed, in which case cargo reruns it anyway.
    // See https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargorerun-if-env-changedname
    println!("cargo:rerun-if-changed=build.rs");
}
