fn main() {
    // Set the host target
    println!(
        "cargo:rustc-env=DOCS_RS_METADATA_HOST_TARGET={}",
        std::env::var("TARGET").unwrap(),
    );
}
