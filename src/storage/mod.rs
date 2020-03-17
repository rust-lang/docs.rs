mod database;
mod s3;

pub(crate) use self::database::DatabaseBackend;
pub(crate) use self::s3::S3Backend;
#[cfg(test)]
pub(crate) use self::s3::TIME_FMT;
use failure::Error;
use time::Timespec;

use std::collections::HashMap;
use std::path::{PathBuf, Path};
use postgres::{Connection, transaction::Transaction};
use std::fs;
use std::io::Read;
use failure::err_msg;
use std::ffi::OsStr;
#[cfg(not(windows))]
use magic::{Cookie, flags};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: String,
    pub(crate) date_updated: Timespec,
    pub(crate) content: Vec<u8>,
}

fn get_file_list_from_dir<P: AsRef<Path>>(path: P,
                                          files: &mut Vec<PathBuf>)
                                          -> Result<(), Error> {
    let path = path.as_ref();

    for file in path.read_dir()? {
        let file = file?;

        if file.file_type()?.is_file() {
            files.push(file.path());
        } else if file.file_type()?.is_dir() {
            get_file_list_from_dir(file.path(), files)?;
        }
    }

    Ok(())
}


pub fn get_file_list<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>, Error> {
    let path = path.as_ref();
    let mut files = Vec::new();

    if !path.exists() {
        return Err(err_msg("File not found"));
    } else if path.is_file() {
        files.push(PathBuf::from(path.file_name().unwrap()));
    } else if path.is_dir() {
        get_file_list_from_dir(path, &mut files)?;
        for file_path in &mut files {
            // We want the paths in this list to not be {path}/bar.txt but just bar.txt
            *file_path = PathBuf::from(file_path.strip_prefix(path).unwrap());
        }
    }

    Ok(files)
}

pub(crate) enum Storage<'a> {
    Database(DatabaseBackend<'a>),
    S3(S3Backend<'a>),
}

impl Storage<'_> {
    pub(crate) fn get(&self, path: &str) -> Result<Blob, Error> {
        match self {
            Self::Database(db) => db.get(path),
            Self::S3(s3) => s3.get(path),
        }
    }

    fn store_batch(&self, batch: &[Blob], trans: &Transaction) -> Result<(), Error> {
        match self {
            Self::Database(db) => db.store_batch(batch, trans),
            Self::S3(s3) => s3.store_batch(batch),
        }
    }

    // Store all files in `root_dir` into the backend under `prefix`.
    //
    // If the environmenet is configured with S3 credentials, this will upload to S3;
    // otherwise, this will store files in the database.
    //
    // This returns a HashMap<filename, mime type>.
    pub(crate) fn store_all(&self, conn: &Connection, prefix: &str, root_dir: &Path) -> Result<HashMap<PathBuf, String>, Error> {
        const MAX_CONCURRENT_UPLOADS: usize = 1000;

        let trans = conn.transaction()?;
        #[cfg(not(windows))]
        let mime_data = load_mime_data()?;
        let mut file_paths_and_mimes = HashMap::new();

        get_file_list(root_dir)?.into_iter()
        .filter_map(|file_path| {
            // Some files have insufficient permissions
            // (like .lock file created by cargo in documentation directory).
            // Skip these files.
            fs::File::open(root_dir.join(&file_path))
                .ok().map(|file| (file_path, file))
        }).map(|(file_path, mut file)| {
            let mut content: Vec<u8> = Vec::new();
            file.read_to_end(&mut content)?;

            let bucket_path = Path::new(prefix).join(&file_path);

            #[cfg(windows)] // On windows, we need to normalize \\ to / so the route logic works
            let bucket_path = path_slash::PathBufExt::to_slash(&bucket_path).unwrap();
            #[cfg(not(windows))]
            let bucket_path = bucket_path.into_os_string().into_string().unwrap();

            #[cfg(windows)]
            let mime = detect_mime(&content, &file_path)?;
            #[cfg(not(windows))]
            let mime = detect_mime(&content, &file_path, &mime_data)?;

            file_paths_and_mimes.insert(file_path, mime.clone());
            Ok(Blob {
                path: bucket_path,
                mime,
                content,
                date_updated: Timespec::new(0, 0),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?
        .chunks(MAX_CONCURRENT_UPLOADS)
        .map(|batch| self.store_batch(batch, &trans))
        // exhaust the iterator
        .for_each(|_| {});

        trans.commit()?;
        Ok(file_paths_and_mimes)
    }
}

#[cfg(not(windows))]
fn load_mime_data() -> Result<Cookie, Error> {
    let cookie = Cookie::open(flags::MIME_TYPE)?;
    cookie.load::<&str>(&[])?;
    Ok(cookie)
}

#[cfg(not(windows))]
fn detect_mime(content: &Vec<u8>, file_path: &Path, cookie: &Cookie) -> Result<String, Error> {
    let mime = cookie.buffer(&content)?;
    correct_mime(&mime, &file_path)
}

#[cfg(windows)]
fn detect_mime(_content: &Vec<u8>, file_path: &Path) -> Result<String, Error> {
    let mime = mime_guess::from_path(file_path).first_raw().map(|m| m).unwrap_or("text/plain");
    correct_mime(&mime, &file_path)
}

fn correct_mime(mime: &str, file_path: &Path) -> Result<String, Error> {
    Ok(match mime {
        "text/plain" | "text/troff" | "text/x-markdown" | "text/x-rust" | "text/x-toml" => {
            match file_path.extension().and_then(OsStr::to_str) {
                Some("md") => "text/markdown",
                Some("rs") => "text/rust",
                Some("markdown") => "text/markdown",
                Some("css") => "text/css",
                Some("toml") => "text/toml",
                Some("js") => "application/javascript",
                Some("json") => "application/json",
                _ => mime
            }
        },
        "image/svg" => "image/svg+xml",
        _ => mime
    }.to_owned())
}

impl<'a> From<DatabaseBackend<'a>> for Storage<'a> {
    fn from(db: DatabaseBackend<'a>) -> Self {
        Self::Database(db)
    }
}

impl<'a> From<S3Backend<'a>> for Storage<'a> {
    fn from(db: S3Backend<'a>) -> Self {
        Self::S3(db)
    }
}

#[cfg(test)]
mod test {
    extern crate env_logger;
    use std::env;
    use crate::test::wrapper;
    use super::*;

    #[test]
    fn test_uploads() {
        use std::fs;
        let dir = tempdir::TempDir::new("docs.rs-upload-test").unwrap();
        let files = ["Cargo.toml", "src/main.rs"];
        for &file in &files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, "data").expect("failed to write to file");
        }
        wrapper(|env| {
            let db = env.db();
            let conn = db.conn();
            let backend = Storage::Database(DatabaseBackend::new(&conn));
            let stored_files = backend.store_all(&conn, "rustdoc", dir.path()).unwrap();
            assert_eq!(stored_files.len(), files.len());
            for name in &files {
                let name = Path::new(name);
                assert!(stored_files.contains_key(name));
            }
            assert_eq!(stored_files.get(Path::new("Cargo.toml")).unwrap(), "text/toml");
            assert_eq!(stored_files.get(Path::new("src/main.rs")).unwrap(), "text/rust");

            let file = backend.get("rustdoc/Cargo.toml").unwrap();
            assert_eq!(file.content, b"data");
            assert_eq!(file.mime, "text/toml");
            assert_eq!(file.path, "rustdoc/Cargo.toml");

            let file = backend.get("rustdoc/src/main.rs").unwrap();
            assert_eq!(file.content, b"data");
            assert_eq!(file.mime, "text/rust");
            assert_eq!(file.path, "rustdoc/src/main.rs");
            Ok(())
        })
    }

    #[test]
    fn test_get_file_list() {
        let _ = env_logger::try_init();

        let files = get_file_list(env::current_dir().unwrap());
        assert!(files.is_ok());
        assert!(files.unwrap().len() > 0);

        let files = get_file_list(env::current_dir().unwrap().join("Cargo.toml")).unwrap();
        assert_eq!(files[0], std::path::Path::new("Cargo.toml"));
    }
    #[test]
    fn test_mime_types() {
        check_mime("/ignored", ".gitignore", "text/plain");
        check_mime("[package]", "hello.toml","text/toml");
        check_mime(".ok { color:red; }", "hello.css","text/css");
        check_mime("var x = 1", "hello.js","application/javascript");
        check_mime("<html>", "hello.html","text/html");
        check_mime("## HELLO", "hello.hello.md","text/markdown");
        check_mime("## WORLD", "hello.markdown","text/markdown");
        check_mime("{}", "hello.json","application/json");
        check_mime("hello world", "hello.txt","text/plain");
        check_mime("//! Simple module to ...", "file.rs", "text/rust");
        check_mime("<svg></svg>", "important.svg", "image/svg+xml");
    }

    fn check_mime(content: &str, path: &str, expected_mime: &str) {
        #[cfg(not(windows))]
        let mime_data = load_mime_data().unwrap();
        #[cfg(windows)]
        let detected_mime = detect_mime(&content.as_bytes().to_vec(), Path::new(&path));
        #[cfg(not(windows))]
        let detected_mime = detect_mime(&content.as_bytes().to_vec(), Path::new(&path), &mime_data);
        let detected_mime = detected_mime.expect("no mime was given");
        assert_eq!(detected_mime, expected_mime);
    }
}
