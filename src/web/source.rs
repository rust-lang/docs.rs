//! Source code browser

use super::file::File as DbFile;
use super::page::Page;
use super::pool::Pool;
use super::MetaData;
use iron::prelude::*;
use postgres::Connection;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::cmp::Ordering;
use std::{collections::BTreeMap, iter::FromIterator};

/// A source file's type
///
/// When serialized into json it takes this form
///
/// * `FileType::Dir`: `{ "file_type_dir": true }`
/// * `FileType::Text`: `{ "file_type_text": true }`
/// * `FileType::Binary`: `{ "file_type_binary": true }`
/// * `FileType::RustSource`: `{ "file_type_rust_source": true }`
///
#[derive(PartialEq, PartialOrd)]
enum FileType {
    Dir,
    Text,
    Binary,
    RustSource,
}

impl ToJson for FileType {
    fn to_json(&self) -> Json {
        let key = match self {
            Self::Dir => "file_type_dir",
            Self::Text => "file_type_text",
            Self::Binary => "file_type_binary",
            Self::RustSource => "file_type_rust_source",
        };

        Json::Object(BTreeMap::from_iter(vec![(
            key.to_string(),
            Json::Boolean(true),
        )]))
    }
}

/// A source file
///
/// Rust:
///
/// ```ignore
/// struct File {
///     name: "main.rs",
///     file_type: FileType::RustSource,
/// }
/// ```
///
/// Json:
///
/// ```json
/// {
///     "name": "main.rs",
///     "file_type_rust_source": true,
/// }
/// ```
///
#[derive(PartialEq, PartialOrd)]
struct File {
    name: String,
    file_type: FileType,
}

impl ToJson for File {
    fn to_json(&self) -> Json {
        let mut file_m: BTreeMap<String, Json> = BTreeMap::new();
        file_m.insert("name".to_string(), self.name.to_json());

        let file_type = match self.file_type {
            FileType::Dir => "file_type_dir",
            FileType::Text => "file_type_text",
            FileType::Binary => "file_type_binary",
            FileType::RustSource => "file_type_rust_source",
        };

        file_m.insert(file_type.to_string(), true.to_json());

        Json::Object(file_m)
    }
}

/// A list of source files
///
///
/// Rust:
///
/// ```ignore
/// FileList {
///     metadata: MetaData {
///         name: "rcc",
///         version: "0.0.0",
///         description: Some("it compiles an unholy language"),
///         target_name: None,
///         rustdoc_status: true,
///         default_target: "x86_64-unknown-linux-gnu",
///     },
///     files: vec![
///         File {
///             name: "main.rs",
///             file_type: FileType::RustSource,
///         },
///         File {
///             name: "lib.rs",
///             file_type: FileType::RustSource,
///         },
///     ],
/// }
/// ```
///
/// Json:
///
/// ```json
/// {
///     "metadata": {
///         "name": "rcc",
///         "version": "0.0.0",
///         "description": "it compiles an unholy language",
///         "target_name": null,
///         "rustdoc_status": true,
///         "default_target": "x86_64-unknown-linux-gnu",
///     },
///     "files": [{
///         "name": "main.rs",
///         "file_type_rust_source": true,
///     }, {
///         "name": "lib.rs",
///         "file_type_rust_source": true,
///     }],
/// }
/// ```
///
struct FileList {
    metadata: MetaData,
    files: Vec<File>,
}

impl ToJson for FileList {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();

        m.insert("metadata".to_string(), self.metadata.to_json());

        // TODO: file_vec from iter

        let mut file_vec: Vec<Json> = Vec::new();

        for file in &self.files {
            let mut file_m: BTreeMap<String, Json> = BTreeMap::new();
            file_m.insert("name".to_string(), file.name.to_json());

            let file_type = match file.file_type {
                FileType::Dir => "file_type_dir",
                FileType::Text => "file_type_text",
                FileType::Binary => "file_type_binary",
                FileType::RustSource => "file_type_rust_source",
            };

            file_m.insert(file_type.to_string(), true.to_json());
            file_vec.push(file_m.to_json());
        }

        m.insert("files".to_string(), file_vec.to_json());
        m.to_json()
    }
}

impl FileList {
    /// Gets FileList from a request path
    ///
    /// All paths stored in database have this format:
    ///
    /// ```text
    /// [
    ///   ["text/plain",".gitignore"],
    ///   ["text/x-c","src/reseeding.rs"],
    ///   ["text/x-c","src/lib.rs"],
    ///   ["text/x-c","README.md"],
    ///   ...
    /// ]
    /// ```
    ///
    /// This function is only returning FileList for requested directory. If is empty,
    /// it will return list of files (and dirs) for root directory. req_path must be a
    /// directory or empty for root directory.
    fn from_path(conn: &Connection, name: &str, version: &str, req_path: &str) -> Option<FileList> {
        let rows = conn
            .query(
                "SELECT crates.name,
                                      releases.version,
                                      releases.description,
                                      releases.target_name,
                                      releases.rustdoc_status,
                                      releases.files,
                                      releases.default_target
                               FROM releases
                               LEFT OUTER JOIN crates ON crates.id = releases.crate_id
                               WHERE crates.name = $1 AND releases.version = $2",
                &[&name, &version],
            )
            .unwrap();

        if rows.is_empty() {
            return None;
        }

        let files: Json = rows.get(0).get_opt(5).unwrap().ok()?;

        let mut file_list: Vec<File> = Vec::new();

        if let Some(files) = files.as_array() {
            for file in files {
                if let Some(file) = file.as_array() {
                    let mime = file[0].as_string().unwrap();
                    let path = file[1].as_string().unwrap();

                    // skip .cargo-ok generated by cargo
                    if path == ".cargo-ok" {
                        continue;
                    }

                    // look only files for req_path
                    if path.starts_with(&req_path) {
                        // remove req_path from path to reach files in this directory
                        let path = path.replace(&req_path, "");
                        let path_splited: Vec<&str> = path.split('/').collect();

                        // if path have '/' it is a directory
                        let ftype = if path_splited.len() > 1 {
                            FileType::Dir
                        } else if mime.starts_with("text") && path_splited[0].ends_with(".rs") {
                            FileType::RustSource
                        } else if mime.starts_with("text") {
                            FileType::Text
                        } else {
                            FileType::Binary
                        };

                        let file = File {
                            name: path_splited[0].to_owned(),
                            file_type: ftype,
                        };

                        // avoid adding duplicates, a directory may occur more than once
                        if !file_list.contains(&file) {
                            file_list.push(file);
                        }
                    }
                }
            }
        }

        if file_list.is_empty() {
            return None;
        }

        file_list.sort_by(|a, b| {
            // directories must be listed first
            if a.file_type == FileType::Dir && b.file_type != FileType::Dir {
                Ordering::Less
            } else if a.file_type != FileType::Dir && b.file_type == FileType::Dir {
                Ordering::Greater
            } else {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
        });

        Some(FileList {
            metadata: MetaData {
                name: rows.get(0).get(0),
                version: rows.get(0).get(1),
                description: rows.get(0).get(2),
                target_name: rows.get(0).get(3),
                rustdoc_status: rows.get(0).get(4),
                default_target: rows.get(0).get(6),
            },
            files: file_list,
        })
    }
}

pub fn source_browser_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(router.find("name"));
    let version = cexpect!(router.find("version"));

    // get path (req_path) for FileList::from_path and actual path for super::file::File::from_path
    let (req_path, file_path) = {
        let mut req_path = req.url.path();
        // remove first elements from path which is /crate/:name/:version/source
        for _ in 0..4 {
            req_path.remove(0);
        }
        let file_path = format!("sources/{}/{}/{}", name, version, req_path.join("/"));

        // FileList::from_path is only working for directories
        // remove file name if it's not a directory
        if let Some(last) = req_path.last_mut() {
            if !last.is_empty() {
                *last = "";
            }
        }

        // remove crate name and version from req_path
        let path = req_path
            .join("/")
            .replace(&format!("{}/{}/", name, version), "");

        (path, file_path)
    };

    let conn = extension!(req, Pool).get();

    // try to get actual file first
    // skip if request is a directory
    let file = if !file_path.ends_with('/') {
        DbFile::from_path(&conn, &file_path)
    } else {
        None
    };

    let (content, is_rust_source) = if let Some(file) = file {
        // serve the file with DatabaseFileHandler if file isn't text and not empty
        if !file.0.mime.starts_with("text") && !file.is_empty() {
            return Ok(file.serve());
        } else if file.0.mime.starts_with("text") && !file.is_empty() {
            (
                String::from_utf8(file.0.content).ok(),
                file.0.path.ends_with(".rs"),
            )
        } else {
            (None, false)
        }
    } else {
        (None, false)
    };

    let list = FileList::from_path(&conn, &name, &version, &req_path);
    if list.is_none() {
        use super::error::Nope;
        use iron::status;
        return Err(IronError::new(Nope::NoResults, status::NotFound));
    }

    let page = Page::new(list)
        .set_bool("show_parent_link", !req_path.is_empty())
        .set_true("javascript_highlightjs")
        .set_true("show_package_navigation")
        .set_true("package_source_tab");

    if let Some(content) = content {
        page.set("file_content", &content)
            .set_bool("file_content_rust_source", is_rust_source)
            .to_resp("source")
    } else {
        page.to_resp("source")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_file_types() {
        assert_eq!(
            Json::from_str(r#"{ "file_type_dir": true }"#).unwrap(),
            FileType::Dir.to_json(),
        );
        assert_eq!(
            Json::from_str(r#"{ "file_type_text": true }"#).unwrap(),
            FileType::Text.to_json(),
        );
        assert_eq!(
            Json::from_str(r#"{ "file_type_binary": true }"#).unwrap(),
            FileType::Binary.to_json(),
        );
        assert_eq!(
            Json::from_str(r#"{ "file_type_rust_source": true }"#).unwrap(),
            FileType::RustSource.to_json(),
        );
    }

    #[test]
    fn serialize_file() {
        assert_eq!(
            Json::from_str(
                r#"{
                    "name": "main.rs",
                    "file_type_rust_source": true
                }"#
            )
            .unwrap(),
            File {
                name: "main.rs".to_string(),
                file_type: FileType::RustSource
            }
            .to_json(),
        );
    }

    #[test]
    fn serialize_file_list() {
        assert_eq!(
            Json::from_str(
                r#"{
                    "metadata": {
                        "name": "rcc",
                        "version": "0.0.0",
                        "description": "it compiles an unholy language",
                        "target_name": null,
                        "rustdoc_status": true,
                        "default_target": "x86_64-unknown-linux-gnu"
                    },
                    "files": [{
                        "name": "main.rs",
                        "file_type_rust_source": true
                    }, {
                        "name": "lib.rs",
                        "file_type_rust_source": true
                    }]
                }"#
            )
            .unwrap(),
            FileList {
                metadata: MetaData {
                    name: "rcc".to_string(),
                    version: "0.0.0".to_string(),
                    description: Some("it compiles an unholy language".to_string()),
                    target_name: None,
                    rustdoc_status: true,
                    default_target: "x86_64-unknown-linux-gnu".to_string(),
                },
                files: vec![
                    File {
                        name: "main.rs".to_string(),
                        file_type: FileType::RustSource
                    },
                    File {
                        name: "lib.rs".to_string(),
                        file_type: FileType::RustSource
                    }
                ],
            }
            .to_json(),
        );
    }
}
