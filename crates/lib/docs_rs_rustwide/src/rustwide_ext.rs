use rustwide::Toolchain;

pub trait ToolchainExt {
    fn default() -> Self;
}

impl ToolchainExt for Toolchain {
    fn default() -> Self {
        Toolchain::dist("nightly")
    }
}
