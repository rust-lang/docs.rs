
use super::DocBuilder;
use super::crates::crates_from_path;
use super::metadata::Metadata;
use utils::{get_package, source_path, copy_doc_dir,
            update_sources, parse_rustc_version, command_result};
use db::{connect_db, add_package_into_database, add_build_into_database, add_path_into_database};
use cargo::core::Package;
use cargo::util::CargoResultExt;
use std::process::Command;
use std::path::PathBuf;
use std::fs::remove_dir_all;
use postgres::Connection;
use rustc_serialize::json::{Json, ToJson};
use error::Result;


/// List of targets supported by docs.rs
const TARGETS: [&'static str; 6] = [
    "i686-apple-darwin",
    "i686-pc-windows-msvc",
    "i686-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu"
];



#[derive(Debug)]
pub struct ChrootBuilderResult {
    pub output: String,
    pub build_success: bool,
    pub have_doc: bool,
    pub have_examples: bool,
    pub rustc_version: String,
    pub cratesfyi_version: String,
}


impl DocBuilder {
    /// Builds every package documentation in chroot environment
    pub fn build_world(&mut self) -> Result<()> {
        try!(update_sources());

        let mut count = 0;

        crates(self.options.crates_io_index_path.clone(), |name, version| {
            match self.build_package(name, version) {
                Ok(status) => {
                    count += 1;
                    if status && count % 10 == 0 {
                        let _ = self.save_cache();
                    }
                }
                Err(err) => warn!("Failed to build package {}-{}: {}", name, version, err),
            }
            self.cache.insert(format!("{}-{}", name, version));
        })
    }


    /// Builds package documentation in chroot environment and adds into cratesfyi database
    pub fn build_package(&mut self, name: &str, version: &str) -> Result<bool> {
        // Skip crates according to options
        if (self.options.skip_if_log_exists &&
            self.cache.contains(&format!("{}-{}", name, version)[..])) ||
           (self.options.skip_if_exists &&
            self.db_cache.contains(&format!("{}-{}", name, version)[..])) {
            return Ok(false);
        }

        info!("Building package {}-{}", name, version);

        // Start with clean documentation directory
        try!(self.remove_build_dir());

        // Database connection
        let conn = try!(connect_db());

        // get_package (and cargo) is using semver, add '=' in front of version.
        let pkg = try!(get_package(name, Some(&format!("={}", version)[..])));
        let metadata = Metadata::from_package(&pkg)?;
        let res = self.build_package_in_chroot(&pkg, metadata.default_target.clone());

        // copy sources and documentation
        let file_list = try!(self.add_sources_into_database(&conn, &pkg));
        let successfully_targets = if res.have_doc {
            let default_target = metadata.default_target.as_ref().map(String::as_str);
            let mut build_targets = metadata.extra_targets.clone().unwrap_or_default();
            if let Some(default_target) = default_target {
                if !build_targets.iter().any(|t| t == default_target) {
                    build_targets.push(default_target.to_owned());
                }
            }
            try!(self.copy_documentation(&pkg,
                                         &res.rustc_version,
                                         default_target,
                                         true));
            let successfully_targets = self.build_package_for_all_targets(&pkg, &build_targets);
            for target in &successfully_targets {
                try!(self.copy_documentation(&pkg, &res.rustc_version, Some(target), false));
            }
            try!(self.add_documentation_into_database(&conn, &pkg));
            successfully_targets
        } else {
            Vec::new()
        };

        let release_id = try!(add_package_into_database(&conn,
                                                        &pkg,
                                                        &res,
                                                        Some(file_list),
                                                        successfully_targets));
        try!(add_build_into_database(&conn, &release_id, &res));

        // remove documentation, source and build directory after we are done
        try!(self.clean(&pkg));

        // add package into build cache
        self.cache.insert(format!("{}-{}", name, version));

        Ok(res.build_success)
    }


    /// Builds documentation of a package with cratesfyi in chroot environment
    fn build_package_in_chroot(&self, package: &Package, default_target: Option<String>) -> ChrootBuilderResult {
        debug!("Building package in chroot");
        let (rustc_version, cratesfyi_version) = self.get_versions();
        let cmd = format!("cratesfyi doc {} ={} {}",
                          package.manifest().name(),
                          package.manifest().version(),
                          default_target.as_ref().unwrap_or(&"".to_string()));
        match self.chroot_command(cmd) {
            Ok(o) => {
                ChrootBuilderResult {
                    output: o,
                    build_success: true,
                    have_doc: self.have_documentation(&package, default_target),
                    have_examples: self.have_examples(&package),
                    rustc_version: rustc_version,
                    cratesfyi_version: cratesfyi_version,
                }
            }
            Err(e) => {
                ChrootBuilderResult {
                    output: e.to_string(),
                    build_success: false,
                    have_doc: false,
                    have_examples: self.have_examples(&package),
                    rustc_version: rustc_version,
                    cratesfyi_version: cratesfyi_version,
                }
            }
        }
    }



    /// Builds documentation of the given crate for the given targets and returns a Vec of the ones
    /// that were successful.
    fn build_package_for_all_targets(&self, package: &Package, targets: &[String]) -> Vec<String> {
        let mut successfuly_targets = Vec::new();

        for target in targets.iter() {
            if !TARGETS.contains(&&**target) {
                // we only support building for win/mac/linux 32-/64-bit
                continue;
            }
            debug!("Building {} for {}", canonical_name(&package), target);
            let cmd = format!("cratesfyi doc {} ={} {}",
                              package.manifest().name(),
                              package.manifest().version(),
                              target);
            if let Ok(_) = self.chroot_command(cmd) {
                // Cargo is not giving any error and not generating documentation of some crates
                // when we use a target compile options. Check documentation exists before
                // adding target to successfully_targets.
                // FIXME: Need to figure out why some docs are not generated with target option
                let target_doc_path = PathBuf::from(&self.options.chroot_path)
                    .join("home")
                    .join(&self.options.chroot_user)
                    .join("cratesfyi")
                    .join(&target)
                    .join("doc");
                if target_doc_path.exists() {
                    successfuly_targets.push(target.to_string());
                }
            }
        }
        successfuly_targets
    }


    /// Copies documentation to destination directory
    fn copy_documentation(&self,
                          package: &Package,
                          rustc_version: &str,
                          target: Option<&str>,
                          is_default_target: bool)
                          -> Result<()> {
        let mut crate_doc_path = PathBuf::from(&self.options.chroot_path)
            .join("home")
            .join(&self.options.chroot_user)
            .join("cratesfyi");

        // docs are available in cratesfyi/$TARGET when target is being used
        if let Some(target) = target {
            crate_doc_path.push(target);
        }

        let mut destination = PathBuf::from(&self.options.destination)
            .join(format!("{}/{}",
                          package.manifest().name(),
                          package.manifest().version()));

        // only add target name to destination directory when we are copying a non-default target.
        // this is allowing us to host documents in the root of the crate documentation directory.
        // for example winapi will be available in docs.rs/winapi/$version/winapi/ for it's
        // default target: x86_64-pc-windows-msvc. But since it will be built under
        // cratesfyi/x86_64-pc-windows-msvc we still need target in this function.
        if !is_default_target {
            if let Some(target) = target {
                destination.push(target);
            }
        }

        copy_doc_dir(crate_doc_path,
                     destination,
                     parse_rustc_version(rustc_version)?.trim())
    }


    /// Removes build directory of a package in chroot
    fn remove_build_dir(&self) -> Result<()> {
        let crate_doc_path = PathBuf::from(&self.options.chroot_path)
            .join("home")
            .join(&self.options.chroot_user)
            .join("cratesfyi")
            .join("doc");
        let _ = remove_dir_all(crate_doc_path);
        for target in TARGETS.iter() {
            let crate_doc_path = PathBuf::from(&self.options.chroot_path)
                .join("home")
                .join(&self.options.chroot_user)
                .join("cratesfyi")
                .join(target)
                .join("doc");
            let _ = remove_dir_all(crate_doc_path);
        }
        Ok(())
    }


    /// Remove documentation, build directory and sources directory of a package
    fn clean(&self, package: &Package) -> Result<()> {
        debug!("Cleaning package");
        use std::fs::remove_dir_all;
        let documentation_path = PathBuf::from(&self.options.destination)
            .join(package.manifest().name().as_str());
        let source_path = source_path(&package).unwrap();
        // Some crates don't have documentation, so we don't care if removing_dir_all fails
        let _ = self.remove_build_dir();
        let _ = remove_dir_all(documentation_path);
        let _ = remove_dir_all(source_path);
        Ok(())
    }


    /// Runs a command in a chroot environment
    fn chroot_command<T: AsRef<str>>(&self, cmd: T) -> Result<String> {
        command_result(Command::new("sudo")
            .arg("lxc-attach")
            .arg("-n")
            .arg(&self.options.container_name)
            .arg("--")
            .arg("su")
            .arg("-")
            .arg(&self.options.chroot_user)
            .arg("-c")
            .arg(cmd.as_ref())
            .output()
            .unwrap())
    }


    /// Checks a package build directory to determine if package have docs
    ///
    /// This function is checking first target in targets to see if documentation exists for a
    /// crate. Package must be successfully built in chroot environment first.
    fn have_documentation(&self, package: &Package, default_target: Option<String>) -> bool {
        let mut crate_doc_path = PathBuf::from(&self.options.chroot_path)
            .join("home")
            .join(&self.options.chroot_user)
            .join("cratesfyi");

        if let Some(default_doc_path) = default_target {
            crate_doc_path.push(default_doc_path);
        }

        crate_doc_path.push("doc");
        crate_doc_path.push(package.targets()[0].name().replace("-", "_").to_string());
        crate_doc_path.exists()
    }


    /// Checks if package have examples
    fn have_examples(&self, package: &Package) -> bool {
        let path = source_path(&package).unwrap().join("examples");
        path.exists() && path.is_dir()
    }


    /// Gets rustc and cratesfyi version from chroot environment
    pub fn get_versions(&self) -> (String, String) {
        // It is safe to use expect here
        // chroot environment must always have rustc and cratesfyi installed
        (String::from(self.chroot_command("rustc --version")
            .expect("Failed to get rustc version")
            .trim()),
         String::from(self.chroot_command("cratesfyi --version")
            .expect("Failed to get cratesfyi version")
            .trim()))
    }


    /// Adds sources into database
    fn add_sources_into_database(&self, conn: &Connection, package: &Package) -> Result<Json> {
        debug!("Adding sources into database");
        let prefix = format!("sources/{}/{}",
                             package.manifest().name(),
                             package.manifest().version());
        add_path_into_database(conn, &prefix, source_path(&package).unwrap())
    }


    /// Adds documentations into database
    fn add_documentation_into_database(&self,
                                       conn: &Connection,
                                       package: &Package)
                                       -> Result<Json> {
        debug!("Adding documentation into database");
        let prefix = format!("rustdoc/{}/{}",
                             package.manifest().name(),
                             package.manifest().version());
        let crate_doc_path = PathBuf::from(&self.options.destination).join(format!("{}/{}",
                          package.manifest().name(),
                          package.manifest().version()));
        add_path_into_database(conn, &prefix, crate_doc_path)
    }


    /// This function will build an empty crate and will add essential documentation files.
    ///
    /// It is required to run after every rustc update. cratesfyi is not keeping this files
    /// for every crate to avoid duplications.
    ///
    /// List of the files:
    ///
    /// * rustdoc.css (with rustc version)
    /// * main.css (with rustc version)
    /// * main.js (with rustc version)
    /// * jquery.js (with rustc version)
    /// * playpen.js (with rustc version)
    /// * normalize.css
    /// * FiraSans-Medium.woff
    /// * FiraSans-Regular.woff
    /// * Heuristica-Italic.woff
    /// * SourceCodePro-Regular.woff
    /// * SourceCodePro-Semibold.woff
    /// * SourceSerifPro-Bold.woff
    /// * SourceSerifPro-Regular.woff
    pub fn add_essential_files(&self) -> Result<()> {
        use std::fs::{copy, create_dir_all};

        // acme-client-0.0.0 is an empty library crate and it will always build
        let pkg = try!(get_package("acme-client", Some("=0.0.0")));
        let res = self.build_package_in_chroot(&pkg, None);
        let rustc_version = parse_rustc_version(&res.rustc_version)?;

        if !res.build_success {
            return Err(format_err!("Failed to build empty crate for: {}", res.rustc_version));
        }

        info!("Copying essential files for: {}", res.rustc_version);

        let files = (// files require rustc version subfix
                     ["brush.svg",
                      "wheel.svg",
                      "down-arrow.svg",
                      "dark.css",
                      "light.css",
                      "main.js",
                      "normalize.css",
                      "rustdoc.css",
                      "settings.css",
                      "settings.js",
                      "storage.js",
                      "theme.js",
                      "source-script.js",
                      "noscript.css",
                      "rust-logo.png"],
                      // favicon.ico is not needed because we set our own
                     // files doesn't require rustc version subfix
                     ["FiraSans-Medium.woff",
                      "FiraSans-Regular.woff",
                      "SourceCodePro-Regular.woff",
                      "SourceCodePro-Semibold.woff",
                      "SourceSerifPro-Bold.ttf.woff",
                      "SourceSerifPro-Regular.ttf.woff",
                      "SourceSerifPro-It.ttf.woff"]);

        let source = PathBuf::from(&self.options.chroot_path)
            .join("home")
            .join(&self.options.chroot_user)
            .join("cratesfyi")
            .join("doc");

        // use copy_documentation destination directory so self.clean can remove it when
        // we are done
        let destination = PathBuf::from(&self.options.destination)
            .join(format!("{}/{}", pkg.manifest().name(), pkg.manifest().version()));
        try!(create_dir_all(&destination));

        for file in files.0.iter() {
            let spl: Vec<&str> = file.split('.').collect();
            let file_name = format!("{}-{}.{}", spl[0], rustc_version, spl[1]);
            let source_path = source.join(&file_name);
            let destination_path = destination.join(&file_name);
            try!(copy(&source_path, &destination_path)
                .chain_err(|| format!("couldn't copy '{}' to '{}'", source_path.display(), destination_path.display())));
        }

        for file in files.1.iter() {
            let source_path = source.join(file);
            let destination_path = destination.join(file);
            try!(copy(&source_path, &destination_path)
                .chain_err(|| format!("couldn't copy '{}' to '{}'", source_path.display(), destination_path.display())));
        }

        let conn = try!(connect_db());
        try!(add_path_into_database(&conn, "", destination));

        try!(self.clean(&pkg));

        let (vers, _) = self.get_versions();

        try!(conn.query("INSERT INTO config (name, value) VALUES ('rustc_version', $1)",
                   &[&vers.to_json()])
            .or_else(|_| {
                conn.query("UPDATE config SET value = $1 WHERE name = 'rustc_version'",
                           &[&vers.to_json()])
            }));

        Ok(())
    }
}


/// Returns canonical name of a package.
///
/// It's just package-version. All directory structure used in cratesfyi is
/// following this naming scheme.
fn canonical_name(package: &Package) -> String {
    format!("{}-{}",
            package.manifest().name(),
            package.manifest().version())
}


/// Runs `func` with the all crates from crates-io.index repository path.
///
/// First argument of func is the name of crate and
/// second argument is the version of crate. Func will be run for every crate.
fn crates<F>(path: PathBuf, mut func: F) -> Result<()>
    where F: FnMut(&str, &str) -> ()
{
    crates_from_path(&path, &mut func)
}


#[cfg(test)]
mod test {
    extern crate env_logger;
    use std::path::PathBuf;
    use {DocBuilder, DocBuilderOptions};

    #[test]
    #[ignore]
    fn test_build_world() {
        let _ = env_logger::try_init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let mut docbuilder = DocBuilder::new(options);
        // This test is building WHOLE WORLD and may take forever
        assert!(docbuilder.build_world().is_ok());
    }

    #[test]
    #[ignore]
    fn test_build_package() {
        let _ = env_logger::try_init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let mut docbuilder = DocBuilder::new(options);
        let res = docbuilder.build_package("rand", "0.3.14");
        assert!(res.is_ok());
    }

    #[test]
    #[ignore]
    fn test_add_essential_files() {
        let _ = env_logger::try_init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let docbuilder = DocBuilder::new(options);

        docbuilder.add_essential_files().unwrap();
    }
}
