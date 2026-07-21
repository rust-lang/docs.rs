use docs_rs_mimes::detect_mime;
use mime::Mime;
use serde_json::Value;
use std::path::PathBuf;

/// represents a file path from our source or documentation builds.
/// Used to return metadata about the file.
#[derive(Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
}

impl FileEntry {
    pub fn mime(&self) -> Mime {
        detect_mime(&self.path)
    }
}

pub fn file_list_to_json(files: impl IntoIterator<Item = FileEntry>) -> Value {
    Value::Array(
        files
            .into_iter()
            .map(|info| {
                Value::Array(vec![
                    Value::String(info.mime().as_ref().to_string()),
                    Value::String(info.path.into_os_string().into_string().unwrap()),
                    Value::Number(info.size.into()),
                ])
            })
            .collect(),
    )
}

#[derive(Debug, Clone, Eq)]
pub enum FolderEntry {
    File(String, Mime),
    Dir(String),
}

impl FolderEntry {
    pub fn from_path(path: &str) -> Self {
        if let Some((dir, _)) = path.split_once('/') {
            Self::Dir(dir.to_string())
        } else {
            Self::File(path.to_string(), detect_mime(path))
        }
    }

    pub fn name(&self) -> &str {
        match self {
            FolderEntry::File(name, _) => name,
            FolderEntry::Dir(name) => name,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Dir(_))
    }

    pub fn mime(&self) -> Option<&Mime> {
        match self {
            Self::File(_, mime) => Some(mime),
            Self::Dir(_) => None,
        }
    }
}

impl PartialEq for FolderEntry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FolderEntry::File(a, _), FolderEntry::File(b, _)) => a == b,
            (FolderEntry::Dir(a), FolderEntry::Dir(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialOrd for FolderEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FolderEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn cmp_name(a: &str, b: &str) -> std::cmp::Ordering {
            a.chars()
                .flat_map(char::to_lowercase)
                .cmp(b.chars().flat_map(char::to_lowercase))
                .then_with(|| a.cmp(b))
        }

        match (self, other) {
            (FolderEntry::Dir(a), FolderEntry::Dir(b)) => cmp_name(a, b),
            (FolderEntry::File(a, _), FolderEntry::File(b, _)) => cmp_name(a, b),
            (FolderEntry::Dir(_), FolderEntry::File(_, _)) => std::cmp::Ordering::Less,
            (FolderEntry::File(_, _), FolderEntry::Dir(_)) => std::cmp::Ordering::Greater,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> FolderEntry {
        FolderEntry::File(name.to_string(), detect_mime(name))
    }

    fn dir(name: &str) -> FolderEntry {
        FolderEntry::Dir(name.to_string())
    }

    #[test]
    fn folder_entry_ordering_sorts_dirs_before_files() {
        let mut entries = vec![
            file("apple.txt"),
            dir("zulu"),
            file("zebra.txt"),
            dir("alpha"),
        ];

        entries.sort();

        assert_eq!(
            entries,
            vec![
                dir("alpha"),
                dir("zulu"),
                file("apple.txt"),
                file("zebra.txt")
            ]
        );
    }

    #[test]
    fn folder_entry_ordering_is_case_insensitive() {
        let mut entries = vec![file("lib.rs"), file("Cargo.toml"), file("apple.txt")];

        entries.sort();

        assert_eq!(
            entries,
            vec![file("apple.txt"), file("Cargo.toml"), file("lib.rs")]
        );
    }

    #[test]
    fn folder_entry_ordering_dirs_is_case_insensitive() {
        let mut entries = vec![dir("src"), dir("Cargo"), dir("apple")];

        entries.sort();

        assert_eq!(entries, vec![dir("apple"), dir("Cargo"), dir("src")]);
    }

    #[test]
    fn folder_entry_ordering_uses_name_tie_breaker() {
        let mut entries = vec![file("readme.md"), file("README.md")];

        entries.sort();

        assert_eq!(entries, vec![file("README.md"), file("readme.md")]);
    }

    #[test]
    fn folder_entry_equality_ignores_mime() {
        assert_eq!(
            FolderEntry::File("same".to_string(), mime::TEXT_PLAIN),
            FolderEntry::File("same".to_string(), mime::TEXT_HTML),
        );
    }

    #[test]
    fn folder_entry_equality_compares_variants_and_names() {
        assert_eq!(dir("src"), dir("src"));
        assert_ne!(dir("src"), dir("lib"));
        assert_ne!(file("src"), dir("src"));
    }
}
