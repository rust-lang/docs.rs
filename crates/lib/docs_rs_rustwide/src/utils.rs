use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

/// Why a check of build output failed.
#[derive(Debug, thiserror::Error)]
pub enum OutputCheckError {
    /// The output is something docs.rs refuses to use: a symlink, a special
    /// file, or a symlinked directory where only plain directories and files
    /// are expected.
    ///
    /// The build can create these from inside the sandbox, pointing at files
    /// on the host.
    #[error("refusing to use untrusted build output {}: {reason}", .path.display())]
    Untrusted { path: PathBuf, reason: &'static str },

    /// The output couldn't be inspected.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<OutputCheckError> for io::Error {
    fn from(err: OutputCheckError) -> Self {
        match err {
            OutputCheckError::Untrusted { .. } => io::Error::new(io::ErrorKind::InvalidData, err),
            OutputCheckError::Io(err) => err,
        }
    }
}

fn untrusted_output(path: &Path, reason: &'static str) -> OutputCheckError {
    OutputCheckError::Untrusted {
        path: path.to_owned(),
        reason,
    }
}

/// Ensure `path` is a directory below `base` that is only reached through real
/// directories, so no component between `base` and `path` is a symlink.
///
/// `base` itself must be trusted. A missing component is reported as an
/// [`OutputCheckError::Io`] error.
pub fn ensure_real_dir_below(base: &Path, path: &Path) -> Result<(), OutputCheckError> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| untrusted_output(path, "not inside the expected directory"))?;

    let mut current = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(untrusted_output(path, "unexpected path component"));
        };
        current.push(part);
        if !fs::symlink_metadata(&current)?.is_dir() {
            return Err(untrusted_output(&current, "not a directory"));
        }
    }
    Ok(())
}

/// Ensure the tree at `root` contains only real directories and regular files.
///
/// Reject symlinks and special files, including `root` itself.
///
/// This is a snapshot. The sandbox container is reused between build steps,
/// so a process the build left behind can still change the tree while the
/// container exists: through the mounted target directory, or possibly
/// through a directory handle it kept open after the tree was moved away.
/// Check again once the container is gone before using the content, like
/// [`copy_dir_all`] does at publication.
pub fn ensure_plain_tree(root: &Path) -> Result<(), OutputCheckError> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(untrusted_output(root, "not a directory"));
    }
    ensure_plain_tree_inner(root)
}

fn ensure_plain_tree_inner(dir: &Path) -> Result<(), OutputCheckError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        // `DirEntry::file_type` doesn't follow symlinks.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            ensure_plain_tree_inner(&entry.path())?;
        } else if !file_type.is_file() {
            return Err(untrusted_output(&entry.path(), "symlink or special file"));
        }
    }
    Ok(())
}

/// Ensure `path` is a regular file, and not a symlink or special file.
///
/// Only reliable when the sandbox can't reach `path`, see [`ensure_plain_tree`].
pub fn ensure_regular_file(path: &Path) -> Result<(), OutputCheckError> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(untrusted_output(path, "not a regular file"));
    }
    Ok(())
}

/// Copy a tree containing only directories and regular files.
///
/// Reject symlinks and special files, including `src` itself.
///
/// This does not protect against concurrent source changes, so it is only
/// reliable when no sandbox container that could reach the source exists
/// anymore. That is the case for documentation at publication, the container
/// is removed when the build finishes. It is not the case for compiler
/// metrics, which are copied while the container still exists.
///
/// The source's parent directories and the destination must be trusted.
/// `on_file` will be called for every copied destination filename.
pub fn copy_dir_all(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    mut on_file: impl FnMut(&Path),
) -> io::Result<()> {
    // Strip trailing slashes and `/.` so symlink_metadata checks the root itself.
    let source = src.as_ref().components().collect::<PathBuf>();
    // `read_dir` follows symlinks, and the entry checks below only cover children.
    if !fs::symlink_metadata(&source)?.is_dir() {
        return Err(untrusted_output(&source, "not a directory").into());
    }
    copy_dir_all_inner(&source, dst.as_ref(), &mut on_file)
}

fn copy_dir_all_inner(src: &Path, dst: &Path, on_file: &mut impl FnMut(&Path)) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all_inner(&entry.path(), &destination_path, on_file)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &destination_path)?;
            on_file(&destination_path);
        } else {
            return Err(untrusted_output(&entry.path(), "symlink or special file").into());
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

        // lets try to copy a src directory to t'p'empdir
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

        for path in copied {
            assert_eq!(fs::read_to_string(path).unwrap(), "<html>spooky</html>");
        }
    }

    fn assert_rejects_untrusted_entry<T>(create_entry: impl FnOnce(&Path, &Path) -> T) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        let secret = root.path().join("host-only-secret");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(&secret, "HOST_SECRET").unwrap();
        let entry = source.join("nested/leak");
        // Keep any returned resource (such as a socket listener) alive while copying.
        let _guard = create_entry(root.path(), &entry);

        let mut copied = Vec::new();
        let error = copy_dir_all(&source, &destination, |p| copied.push(p.to_owned())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(copied.is_empty());
        assert!(!destination.join("nested/leak").exists());
    }

    #[test]
    fn rejects_absolute_file_symlink() {
        assert_rejects_untrusted_entry(|root, entry| {
            std::os::unix::fs::symlink(root.join("host-only-secret"), entry).unwrap();
        });
    }

    #[test]
    fn rejects_directory_symlink() {
        assert_rejects_untrusted_entry(|root, entry| {
            std::os::unix::fs::symlink(root, entry).unwrap();
        });
    }

    #[test]
    fn rejects_unix_socket() {
        assert_rejects_untrusted_entry(|_, entry| {
            std::os::unix::net::UnixListener::bind(entry).unwrap()
        });
    }

    #[test_case("source"; "plain root")]
    #[test_case("source/"; "root with trailing slash")]
    #[test_case("source/."; "root with trailing dot")]
    fn rejects_symlink_source_root(path: &str) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::os::unix::fs::symlink(root.path(), &source).unwrap();
        assert!(
            copy_dir_all(
                root.path().join(path),
                root.path().join("destination"),
                |_| { panic!("must not copy from a symlink root") }
            )
            .is_err()
        );
    }

    #[test]
    fn real_dir_below_accepts_plain_directories() {
        let base = tempfile::tempdir().unwrap();
        let doc = base.path().join("x86_64-unknown-linux-gnu/doc");
        fs::create_dir_all(&doc).unwrap();
        ensure_real_dir_below(base.path(), &doc).unwrap();
    }

    #[test_case(true; "symlinked target dir")]
    #[test_case(false; "symlinked doc dir")]
    fn real_dir_below_rejects_symlinked_directories(symlinked_target_dir: bool) {
        let host = tempfile::tempdir().unwrap();
        fs::create_dir_all(host.path().join("doc")).unwrap();

        let base = tempfile::tempdir().unwrap();
        let target_dir = base.path().join("x86_64-unknown-linux-gnu");
        if symlinked_target_dir {
            std::os::unix::fs::symlink(host.path(), &target_dir).unwrap();
        } else {
            fs::create_dir(&target_dir).unwrap();
            std::os::unix::fs::symlink(host.path().join("doc"), target_dir.join("doc")).unwrap();
        }

        let err = ensure_real_dir_below(base.path(), &target_dir.join("doc")).unwrap_err();
        assert!(matches!(err, OutputCheckError::Untrusted { .. }));
    }

    #[test]
    fn real_dir_below_reports_missing_directories_as_not_found() {
        let base = tempfile::tempdir().unwrap();
        let err = ensure_real_dir_below(base.path(), &base.path().join("missing/doc")).unwrap_err();
        assert!(matches!(err, OutputCheckError::Io(err) if err.kind() == io::ErrorKind::NotFound));
    }

    #[test]
    fn regular_file_rejects_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("output.json");
        fs::write(&file, "{}").unwrap();
        ensure_regular_file(&file).unwrap();

        let link = root.path().join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let err = ensure_regular_file(&link).unwrap_err();
        assert!(matches!(err, OutputCheckError::Untrusted { .. }));
    }

    #[test]
    fn plain_tree_accepts_directories_and_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("a/b")).unwrap();
        fs::write(root.path().join("a/b/index.html"), "<html></html>").unwrap();
        ensure_plain_tree(root.path()).unwrap();
    }

    #[test_case(false; "nested symlink")]
    #[test_case(true; "symlinked root")]
    fn plain_tree_rejects_symlinks(symlinked_root: bool) {
        let host = tempfile::tempdir().unwrap();
        fs::write(host.path().join("secret"), "HOST_SECRET").unwrap();

        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("docs");
        if symlinked_root {
            std::os::unix::fs::symlink(host.path(), &root).unwrap();
        } else {
            fs::create_dir_all(root.join("nested")).unwrap();
            std::os::unix::fs::symlink(host.path().join("secret"), root.join("nested/leak"))
                .unwrap();
        }

        let err = ensure_plain_tree(&root).unwrap_err();
        assert!(matches!(err, OutputCheckError::Untrusted { .. }));
    }
}
