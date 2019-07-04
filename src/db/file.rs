//! Simple module to store files in database.
//!
//! cratesfyi is generating more than 5 million files, they are small and mostly html files.
//! They are using so many inodes and it is better to store them in database instead of
//! filesystem. This module is adding files into database and retrieving them.


use std::path::{PathBuf, Path};
use postgres::Connection;
use rustc_serialize::json::{Json, ToJson};
use std::fs;
use std::io::Read;
use error::Result;
use failure::err_msg;
use rusoto_s3::{S3, PutObjectRequest, GetObjectRequest, S3Client};
use rusoto_core::region::Region;
use rusoto_credential::EnvironmentProvider;


fn get_file_list_from_dir<P: AsRef<Path>>(path: P,
                                          files: &mut Vec<PathBuf>)
                                          -> Result<()> {
    let path = path.as_ref();

    for file in try!(path.read_dir()) {
        let file = try!(file);

        if try!(file.file_type()).is_file() {
            files.push(file.path());
        } else if try!(file.file_type()).is_dir() {
            try!(get_file_list_from_dir(file.path(), files));
        }
    }

    Ok(())
}


pub fn get_file_list<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>> {
    let path = path.as_ref();
    let mut files = Vec::new();

    if !path.exists() {
        return Err(err_msg("File not found"));
    } else if path.is_file() {
        files.push(PathBuf::from(path.file_name().unwrap()));
    } else if path.is_dir() {
        try!(get_file_list_from_dir(path, &mut files));
        for file_path in &mut files {
            // We want the paths in this list to not be {path}/bar.txt but just bar.txt
            *file_path = PathBuf::from(file_path.strip_prefix(path).unwrap());
        }
    }

    Ok(files)
}

pub struct Blob {
    pub path: String,
    pub mime: String,
    pub date_updated: time::Timespec,
    pub content: Vec<u8>,
}

pub fn get_path(conn: &Connection, path: &str) -> Option<Blob> {
    let rows = conn.query("SELECT path, mime, date_updated, content
                           FROM files
                           WHERE path = $1", &[&path]).unwrap();

    if rows.len() == 0 {
        None
    } else {
        let client = s3_client();
        let row = rows.get(0);
        let content = client.get_object(GetObjectRequest {
            bucket: "rust-docs-rs".into(),
            key: path.into(),
            ..Default::default()
        }).sync().ok().and_then(|r| r.body).map(|b| {
            let mut b = b.into_blocking_read();
            let mut content = Vec::new();
            b.read_to_end(&mut content).unwrap();
            content
        }).unwrap_or(row.get(3));

        Some(Blob {
            path: row.get(0),
            mime: row.get(1),
            date_updated: row.get(2),
            content,
        })
    }
}

fn s3_client() -> S3Client {
    let client = S3Client::new_with(
        rusoto_core::request::HttpClient::new().unwrap(),
        EnvironmentProvider::default(),
        Region::UsWest1,
    );
    client
}

/// Adds files into database and returns list of files with their mime type in Json
pub fn add_path_into_database<P: AsRef<Path>>(conn: &Connection,
                                              prefix: &str,
                                              path: P)
                                              -> Result<Json> {
    use magic::{Cookie, flags};
    let cookie = try!(Cookie::open(flags::MIME_TYPE));
    try!(cookie.load::<&str>(&[]));

    let trans = try!(conn.transaction());
    let client = s3_client();
    let mut file_list_with_mimes: Vec<(String, PathBuf)> = Vec::new();

    for file_path in try!(get_file_list(&path)) {
        let (path, mime) = {
            let path = Path::new(path.as_ref()).join(&file_path);
            // Some files have insufficient permissions (like .lock file created by cargo in
            // documentation directory). We are skipping this files.
            let mut file = match fs::File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut content: Vec<u8> = Vec::new();
            try!(file.read_to_end(&mut content));
            let bucket_path = Path::new(prefix).join(&file_path)
                .into_os_string().into_string().unwrap();

            let mime = {
                let mime = try!(cookie.buffer(&content));
                // css's are causing some problem in browsers
                // magic will return text/plain for css file types
                // convert them to text/css
                // do the same for javascript files
                if mime == "text/plain" {
                    let e = file_path.extension().unwrap_or_default();
                    if e == "css" {
                        "text/css".to_owned()
                    } else if e == "js" {
                        "application/javascript".to_owned()
                    } else {
                        mime.to_owned()
                    }
                } else {
                    mime.to_owned()
                }
            };

            client.put_object(PutObjectRequest {
                acl: Some("public-read".into()),
                bucket: "rust-docs-rs".into(),
                key: bucket_path.clone(),
                body: Some(content.clone().into()),
                content_type: Some(mime.clone()),
                ..Default::default()
            }).sync().unwrap();

            file_list_with_mimes.push((mime.clone(), file_path.clone()));

            (
                bucket_path,
                mime,
            )
        };

        // check if file already exists in database
        let rows = try!(conn.query("SELECT COUNT(*) FROM files WHERE path = $1", &[&path]));

        if rows.get(0).get::<usize, i64>(0) == 0 {
            try!(trans.query("INSERT INTO files (path, mime, content) VALUES ($1, $2, $3)",
                             &[&path, &mime, &"in-s3".to_owned()]));
        } else {
            try!(trans.query("UPDATE files SET mime = $2, date_updated = NOW() \
                              WHERE path = $1",
                             &[&path, &mime]));
        }
    }

    try!(trans.commit());

    file_list_to_json(file_list_with_mimes)
}



fn file_list_to_json(file_list: Vec<(String, PathBuf)>) -> Result<Json> {

    let mut file_list_json: Vec<Json> = Vec::new();

    for file in file_list {
        let mut v: Vec<String> = Vec::new();
        v.push(file.0.clone());
        v.push(file.1.into_os_string().into_string().unwrap());
        file_list_json.push(v.to_json());
    }

    Ok(file_list_json.to_json())
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use std::env;
    use super::get_file_list;

    #[test]
    fn test_get_file_list() {
        let _ = env_logger::try_init();

        let files = get_file_list(env::current_dir().unwrap());
        assert!(files.is_ok());
        assert!(files.unwrap().len() > 0);

        let files = get_file_list(env::current_dir().unwrap().join("Cargo.toml")).unwrap();
        assert_eq!(files[0], std::path::Path::new("Cargo.toml"));
    }
}
