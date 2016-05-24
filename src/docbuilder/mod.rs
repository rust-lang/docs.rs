

pub mod error;
pub mod options;
mod chroot_builder;
mod crates;

pub use self::chroot_builder::ChrootBuilderResult;

use ::{DocBuilderError, DocBuilderOptions};
use self::crates::crates_from_path;


/// chroot based documentation builder
pub struct DocBuilder {
    options: DocBuilderOptions
}


impl DocBuilder {
    pub fn new(options: DocBuilderOptions) -> DocBuilder {
        DocBuilder {
            options: options
        }
    }


    /// Runs `func` with the all crates from crates-io.index repository.
    ///
    /// First argument of func is the name of crate and
    /// second argument is the version of crate. Func will be run for every crate.
    fn crates<F>(&self, func: F) -> Result<(), DocBuilderError>
        where F: Fn(&str, &str) -> () {
        crates_from_path(&self.options.crates_io_index_path, &func)
    }
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use ::DocBuilderOptions;
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore]
    fn test_docbuilder_crates() {
        let _ = env_logger::init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let docbuilder = DocBuilder::new(options);
        let res = docbuilder.crates(|_, _| {});
        assert!(res.is_ok());
    }
}
