mod database;
pub(crate) mod s3;

pub(crate) use self::database::DatabaseBackend;
pub(crate) use self::s3::S3Backend;
use chrono::{DateTime, Utc};
use failure::{err_msg, Error};
use postgres::{transaction::Transaction, Connection};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_CONCURRENT_UPLOADS: usize = 1000;
const DEFAULT_COMPRESSION: CompressionAlgorithm = CompressionAlgorithm::Zstd;

pub type CompressionAlgorithms = HashSet<CompressionAlgorithm>;

macro_rules! enum_id {
    ($vis:vis enum $name:ident { $($variant:ident = $discriminant:expr,)* }) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        $vis enum $name {
            $($variant = $discriminant,)*
        }

        impl $name {
            #[cfg(test)]
            const AVAILABLE: &'static [Self] = &[$(Self::$variant,)*];
        }

        impl fmt::Display for CompressionAlgorithm {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $(Self::$variant => write!(f, stringify!($variant)),)*
                }
            }
        }

        impl std::str::FromStr for CompressionAlgorithm {
            type Err = ();
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err(()),
                }
            }
        }

        impl std::convert::TryFrom<i32> for CompressionAlgorithm {
            type Error = i32;
            fn try_from(i: i32) -> Result<Self, Self::Error> {
                match i {
                    $($discriminant => Ok(Self::$variant),)*
                    _ => Err(i),
                }
            }
        }
    }
}

enum_id! {
    pub enum CompressionAlgorithm {
        Zstd = 0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: String,
    pub(crate) date_updated: DateTime<Utc>,
    pub(crate) content: Vec<u8>,
    pub(crate) compression: Option<CompressionAlgorithm>,
}

fn get_file_list_from_dir<P: AsRef<Path>>(path: P, files: &mut Vec<PathBuf>) -> Result<(), Error> {
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

impl<'a> Storage<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Self {
        if let Some(c) = s3::s3_client() {
            Storage::from(S3Backend::new(c, s3::S3_BUCKET_NAME))
        } else {
            DatabaseBackend::new(conn).into()
        }
    }
    pub(crate) fn get(&self, path: &str, max_size: usize) -> Result<Blob, Error> {
        let mut blob = match self {
            Self::Database(db) => db.get(path, max_size),
            Self::S3(s3) => s3.get(path, max_size),
        }?;
        if let Some(alg) = blob.compression {
            blob.content = decompress(blob.content.as_slice(), alg, max_size)?;
            blob.compression = None;
        }
        Ok(blob)
    }

    fn store_batch(&mut self, batch: &[Blob], trans: &Transaction) -> Result<(), Error> {
        match self {
            Self::Database(db) => db.store_batch(batch, trans),
            Self::S3(s3) => s3.store_batch(batch),
        }
    }

    // Store all files in `root_dir` into the backend under `prefix`.
    //
    // If the environment is configured with S3 credentials, this will upload to S3;
    // otherwise, this will store files in the database.
    //
    // This returns (map<filename, mime type>, set<compression algorithms>).
    pub(crate) fn store_all(
        &mut self,
        conn: &Connection,
        prefix: &str,
        root_dir: &Path,
    ) -> Result<(HashMap<PathBuf, String>, HashSet<CompressionAlgorithm>), Error> {
        let trans = conn.transaction()?;
        let mut file_paths_and_mimes = HashMap::new();
        let mut algs = HashSet::with_capacity(1);

        let mut blobs = get_file_list(root_dir)?
            .into_iter()
            .filter_map(|file_path| {
                // Some files have insufficient permissions
                // (like .lock file created by cargo in documentation directory).
                // Skip these files.
                fs::File::open(root_dir.join(&file_path))
                    .ok()
                    .map(|file| (file_path, file))
            })
            .map(|(file_path, file)| -> Result<_, Error> {
                let alg = DEFAULT_COMPRESSION;
                let content = compress(file, alg)?;
                let bucket_path = Path::new(prefix).join(&file_path);

                #[cfg(windows)] // On windows, we need to normalize \\ to / so the route logic works
                let bucket_path = path_slash::PathBufExt::to_slash(&bucket_path).unwrap();
                #[cfg(not(windows))]
                let bucket_path = bucket_path.into_os_string().into_string().unwrap();

                let mime = detect_mime(&file_path)?;
                file_paths_and_mimes.insert(file_path, mime.to_string());
                algs.insert(alg);

                Ok(Blob {
                    path: bucket_path,
                    mime: mime.to_string(),
                    content,
                    compression: Some(alg),
                    // this field is ignored by the backend
                    date_updated: Utc::now(),
                })
            });
        loop {
            let batch: Vec<_> = blobs
                .by_ref()
                .take(MAX_CONCURRENT_UPLOADS)
                .collect::<Result<_, Error>>()?;
            if batch.is_empty() {
                break;
            }
            self.store_batch(&batch, &trans)?;
        }

        trans.commit()?;
        Ok((file_paths_and_mimes, algs))
    }
}

// public for benchmarking
pub fn compress(content: impl Read, algorithm: CompressionAlgorithm) -> Result<Vec<u8>, Error> {
    match algorithm {
        CompressionAlgorithm::Zstd => Ok(zstd::encode_all(content, 9)?),
    }
}

pub fn decompress(
    content: impl Read,
    algorithm: CompressionAlgorithm,
    max_size: usize,
) -> Result<Vec<u8>, Error> {
    // The sized buffer prevents a malicious file from decompressing to multiple times its size.
    let mut buffer = crate::utils::sized_buffer::SizedBuffer::new(max_size);

    match algorithm {
        CompressionAlgorithm::Zstd => zstd::stream::copy_decode(content, &mut buffer)?,
    }

    Ok(buffer.into_inner())
}

fn detect_mime(file_path: &Path) -> Result<&'static str, Error> {
    let mime = mime_guess::from_path(file_path)
        .first_raw()
        .map(|m| m)
        .unwrap_or("text/plain");
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
                _ => mime,
            }
        }
        "image/svg" => "image/svg+xml",
        _ => mime,
    })
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
    use super::*;
    use crate::test::wrapper;
    use std::env;

    pub(crate) fn assert_blob_eq(blob: &Blob, actual: &Blob) {
        assert_eq!(blob.path, actual.path);
        assert_eq!(blob.content, actual.content);
        assert_eq!(blob.mime, actual.mime);
        // NOTE: this does _not_ compare the upload time since min.io doesn't allow this to be configured
    }

    pub(crate) fn test_roundtrip(blobs: &[Blob]) {
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-test")
            .tempdir()
            .unwrap();
        for blob in blobs {
            let path = dir.path().join(&blob.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, &blob.content).expect("failed to write to file");
        }
        wrapper(|env| {
            let db = env.db();
            let conn = db.conn();
            let mut backend = Storage::Database(DatabaseBackend::new(&conn));
            let (stored_files, _algs) = backend.store_all(&conn, "", dir.path()).unwrap();
            assert_eq!(stored_files.len(), blobs.len());
            for blob in blobs {
                let name = Path::new(&blob.path);
                assert!(stored_files.contains_key(name));

                let actual = backend.get(&blob.path, std::usize::MAX).unwrap();
                assert_blob_eq(blob, &actual);
            }

            Ok(())
        });
    }

    #[test]
    fn test_uploads() {
        use std::fs;
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-test")
            .tempdir()
            .unwrap();
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
            let mut backend = Storage::Database(DatabaseBackend::new(&conn));
            let (stored_files, _algs) = backend.store_all(&conn, "rustdoc", dir.path()).unwrap();
            assert_eq!(stored_files.len(), files.len());
            for name in &files {
                let name = Path::new(name);
                assert!(stored_files.contains_key(name));
            }
            assert_eq!(
                stored_files.get(Path::new("Cargo.toml")).unwrap(),
                "text/toml"
            );
            assert_eq!(
                stored_files.get(Path::new("src/main.rs")).unwrap(),
                "text/rust"
            );

            let file = backend.get("rustdoc/Cargo.toml", std::usize::MAX).unwrap();
            assert_eq!(file.content, b"data");
            assert_eq!(file.mime, "text/toml");
            assert_eq!(file.path, "rustdoc/Cargo.toml");

            let file = backend.get("rustdoc/src/main.rs", std::usize::MAX).unwrap();
            assert_eq!(file.content, b"data");
            assert_eq!(file.mime, "text/rust");
            assert_eq!(file.path, "rustdoc/src/main.rs");
            Ok(())
        })
    }

    #[test]
    fn test_batched_uploads() {
        let uploads: Vec<_> = (0..=MAX_CONCURRENT_UPLOADS + 1)
            .map(|i| {
                let alg = DEFAULT_COMPRESSION;
                let content = compress("fn main() {}".as_bytes(), alg).unwrap();
                Blob {
                    mime: "text/rust".into(),
                    content,
                    path: format!("{}.rs", i),
                    date_updated: Utc::now(),
                    compression: Some(alg),
                }
            })
            .collect();

        test_roundtrip(&uploads);
    }

    #[test]
    fn test_get_file_list() {
        crate::test::init_logger();
        let files = get_file_list(env::current_dir().unwrap());
        assert!(files.is_ok());
        assert!(files.unwrap().len() > 0);

        let files = get_file_list(env::current_dir().unwrap().join("Cargo.toml")).unwrap();
        assert_eq!(files[0], std::path::Path::new("Cargo.toml"));
    }

    #[test]
    fn test_compression() {
        let orig = "fn main() {}";
        for alg in CompressionAlgorithm::AVAILABLE {
            println!("testing algorithm {}", alg);

            let data = compress(orig.as_bytes(), *alg).unwrap();
            let blob = Blob {
                mime: "text/rust".into(),
                content: data.clone(),
                path: "main.rs".into(),
                date_updated: Utc::now(),
                compression: Some(*alg),
            };
            test_roundtrip(std::slice::from_ref(&blob));
            assert_eq!(
                decompress(data.as_slice(), *alg, std::usize::MAX).unwrap(),
                orig.as_bytes()
            );
        }
    }

    #[test]
    fn test_decompression_too_big() {
        let orginal = &[b'A'; 2000] as &[u8];

        for alg in CompressionAlgorithm::AVAILABLE {
            let data = compress(orginal, *alg).unwrap();

            // Ensure decompressing a file over the limit returns a SizeLimitReached error.
            let err = decompress(data.as_slice(), *alg, 1000).unwrap_err();
            assert!(err
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                .is_some());
        }
    }

    #[test]
    fn test_mime_types() {
        check_mime(".gitignore", "text/plain");
        check_mime("hello.toml", "text/toml");
        check_mime("hello.css", "text/css");
        check_mime("hello.js", "application/javascript");
        check_mime("hello.html", "text/html");
        check_mime("hello.hello.md", "text/markdown");
        check_mime("hello.markdown", "text/markdown");
        check_mime("hello.json", "application/json");
        check_mime("hello.txt", "text/plain");
        check_mime("file.rs", "text/rust");
        check_mime("important.svg", "image/svg+xml");
    }

    fn check_mime(path: &str, expected_mime: &str) {
        let detected_mime = detect_mime(Path::new(&path));
        let detected_mime = detected_mime.expect("no mime was given");
        assert_eq!(detected_mime, expected_mime);
    }

    #[test]
    fn test_compression_try_from_is_exhaustive() {
        use std::convert::TryFrom;

        let a = CompressionAlgorithm::Zstd;
        match a {
            CompressionAlgorithm::Zstd => {
                assert_eq!(a, CompressionAlgorithm::try_from(a as i32).unwrap());
                assert_eq!(a, a.to_string().parse().unwrap());
            }
        }
    }
}
