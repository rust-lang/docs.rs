use anyhow::Result;
use async_stream::try_stream;
use futures_util::Stream;
use std::{
    fs::Metadata,
    io, iter,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub fn get_file_list<P: AsRef<Path>>(path: P) -> Box<dyn Iterator<Item = Result<PathBuf>>> {
    let path = path.as_ref().to_path_buf();
    if path.is_file() {
        let path = if let Some(parent) = path.parent() {
            path.strip_prefix(parent).unwrap().to_path_buf()
        } else {
            path
        };

        Box::new(iter::once(Ok(path)))
    } else if path.is_dir() {
        Box::new(
            WalkDir::new(path.clone())
                .into_iter()
                .filter_map(move |result| {
                    let direntry = match result {
                        Ok(de) => de,
                        Err(err) => return Some(Err(err.into())),
                    };

                    if !direntry.file_type().is_dir() {
                        Some(Ok(direntry
                            .path()
                            .strip_prefix(&path)
                            .unwrap()
                            .to_path_buf()))
                    } else {
                        None
                    }
                }),
        )
    } else {
        Box::new(iter::empty())
    }
}

#[derive(Debug)]
pub(crate) struct FileItem {
    pub(crate) absolute: PathBuf,
    pub(crate) relative: PathBuf,
    pub(crate) metadata: Metadata,
}

impl AsRef<Path> for FileItem {
    fn as_ref(&self) -> &Path {
        &self.absolute
    }
}

/// Recursively walks a directory and yields all files (not directories) found within it.
///
/// Roughly an async version of `get_file_list`.
pub(crate) fn walk_dir_recursive(
    root: impl AsRef<Path>,
) -> impl Stream<Item = Result<FileItem, io::Error>> {
    let root = root.as_ref().to_path_buf();
    try_stream! {
        let mut dirs = vec![root.clone()];

        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let meta = entry.metadata().await?;
                if meta.is_dir() {
                    dirs.push(path.clone());
                } else {
                    let relative =  path.strip_prefix(&root).unwrap().to_path_buf();

                    yield FileItem { absolute: path, relative, metadata: meta };
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures_util::TryStreamExt;

    #[test]
    fn test_get_file_list() -> Result<()> {
        use std::fs;

        let dir = tempfile::tempdir()?;
        let root = dir.path();

        fs::create_dir_all(root.join("nested"))?;
        fs::write(root.join("root.txt"), b"root")?;
        fs::write(root.join("nested").join("child.txt"), b"child")?;

        let mut files: Vec<_> = get_file_list(root).collect::<Result<Vec<_>>>()?;
        files.sort();
        assert_eq!(
            files,
            vec![PathBuf::from("nested/child.txt"), PathBuf::from("root.txt"),]
        );

        let files: Vec<_> = get_file_list(root.join("root.txt")).collect::<Result<Vec<_>>>()?;
        assert_eq!(files, vec![PathBuf::from("root.txt")]);

        Ok(())
    }

    #[tokio::test]
    async fn test_walk_dir_recursive() -> Result<()> {
        use tokio::fs;

        let dir = tempfile::tempdir()?;
        let root = dir.path();

        let nested = root.join("a/b");
        fs::create_dir_all(&nested).await?;
        fs::write(root.join("root.txt"), b"root").await?;
        fs::write(root.join("a").join("child.txt"), b"child").await?;
        fs::write(nested.join("leaf.txt"), b"leaf").await?;

        let mut files: Vec<_> = walk_dir_recursive(root)
            .map_ok(|item| item.relative)
            .try_collect()
            .await?;
        files.sort();

        assert_eq!(
            files,
            vec![
                PathBuf::from("a/b/leaf.txt"),
                PathBuf::from("a/child.txt"),
                PathBuf::from("root.txt"),
            ]
        );

        Ok(())
    }
}
