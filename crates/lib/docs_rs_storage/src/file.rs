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
