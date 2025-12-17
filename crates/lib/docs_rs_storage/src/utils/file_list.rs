use anyhow::Result;
use std::{
    iter,
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

#[cfg(test)]
mod test {
    use super::*;
    use std::env;

    #[test]
    fn test_get_file_list() -> Result<()> {
        let dir = env::current_dir().unwrap();

        let files: Vec<_> = get_file_list(&dir).collect::<Result<Vec<_>>>()?;
        assert!(!files.is_empty());

        let files: Vec<_> = get_file_list(dir.join("Cargo.toml")).collect::<Result<Vec<_>>>()?;
        assert_eq!(files[0], std::path::Path::new("Cargo.toml"));

        Ok(())
    }
}
