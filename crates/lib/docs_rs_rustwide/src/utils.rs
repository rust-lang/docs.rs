use anyhow::Result;
use rustwide::cmd::{CommandError, SandboxImage};
use std::{fs, io, path::Path};
use tracing::{debug, instrument};

/// cp -r src dst
///
/// `on_file` will be called for every destination filename
pub fn copy_dir_all(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    mut on_file: impl FnMut(&Path),
) -> io::Result<()> {
    copy_dir_all_inner(src.as_ref(), dst.as_ref(), &mut on_file)
}

fn copy_dir_all_inner(
    src: &Path,
    dst: &Path,
    on_file: &mut impl FnMut(&Path),
) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let filename = entry.file_name();
        if entry.file_type()?.is_dir() {
            copy_dir_all_inner(&entry.path(), &dst.join(filename), on_file)?;
        } else {
            let destination_path = dst.join(filename);
            fs::copy(entry.path(), &destination_path)?;
            on_file(&destination_path);
        }
    }
    Ok(())
}

pub(crate) fn args_contain_unstable_feature<S>(
    cargo_args: impl IntoIterator<Item = S>,
    feature: &str,
) -> bool
where
    S: AsRef<str>,
{
    let mut cargo_args = cargo_args.into_iter().peekable();

    while let Some(arg) = cargo_args.next() {
        let arg = arg.as_ref();
        if arg
            .strip_prefix("-Z")
            .is_some_and(|value| unstable_feature_matches(value, feature))
        {
            return true;
        }

        if arg == "-Z"
            && cargo_args
                .peek()
                .is_some_and(|next| unstable_feature_matches(next.as_ref(), feature))
        {
            return true;
        }
    }

    false
}

fn unstable_feature_matches(value: &str, feature: &str) -> bool {
    value == feature
        || value
            .strip_prefix(feature)
            .is_some_and(|suffix| suffix.starts_with('='))
}

/// Resolve a sandbox image name, preferring an existing local image and
/// falling back to a remote image that rustwide will pull when needed.
#[instrument(skip_all, fields(image = name))]
pub fn resolve_sandbox_image(name: &str) -> Result<SandboxImage> {
    match SandboxImage::local(name) {
        Ok(image) => {
            debug!("using local sandbox image");
            Ok(image)
        }
        Err(CommandError::SandboxImageMissing(_)) => {
            debug!("local sandbox image is missing; resolving remote image");
            Ok(SandboxImage::remote(name)?)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use test_case::test_case;

    #[test_case(&[], "build-std" => false; "empty arguments")]
    #[test_case(&["-Zbuild-std"], "build-std" => true; "joined flag")]
    #[test_case(&["-Zbuild-std=core,alloc"], "build-std" => true; "joined flag with value")]
    #[test_case(&["-Z", "build-std"], "build-std" => true; "split flag")]
    #[test_case(&["-Z", "build-std=core,alloc"], "build-std" => true; "split flag with value")]
    #[test_case(&["rustdoc", "-Zbuild-std", "--lib"], "build-std" => true; "among other arguments")]
    #[test_case(&["build-std"], "build-std" => false; "missing z prefix")]
    #[test_case(&["-Z"], "build-std" => false; "z without feature")]
    #[test_case(&["-Zunstable-options"], "build-std" => false; "different feature")]
    #[test_case(&["-Zbuild-stdlib"], "build-std" => false; "feature name prefix")]
    #[test_case(&["-Zbuild-std-extra"], "build-std" => false; "feature name with suffix")]
    #[test_case(&["-Z", "build-stdlib"], "build-std" => false; "split feature name prefix")]
    #[test_case(&["-Zunstable-options"], "unstable-options" => true; "generic feature name")]
    fn detects_unstable_feature(args: &[&str], feature: &str) -> bool {
        args_contain_unstable_feature(args.iter().copied(), feature)
    }

    #[test]
    fn test_copy_doc_dir() {
        use pretty_assertions::assert_eq;

        let source = tempfile::Builder::new()
            .prefix("docsrs-src")
            .tempdir()
            .unwrap();
        let destination = tempfile::Builder::new()
            .prefix("docsrs-dst")
            .tempdir()
            .unwrap();
        let doc = source.path().join("doc");
        fs::create_dir(&doc).unwrap();
        fs::create_dir(doc.join("inner")).unwrap();

        fs::write(doc.join("index.html"), "<html>spooky</html>").unwrap();
        fs::write(doc.join("inner").join("index.html"), "<html>spooky</html>").unwrap();

        // lets try to copy a src directory to tempdir
        let mut copied = Vec::new();
        copy_dir_all(source.path().join("doc"), destination.path(), |path| {
            copied.push(PathBuf::from(path));
        })
        .unwrap();

        copied.sort();

        assert_eq!(
            copied,
            vec![
                destination.path().join("index.html"),
                destination.path().join("inner").join("index.html"),
            ]
        );

        assert!(copied.iter().all(|p| p.exists()));
    }
}
