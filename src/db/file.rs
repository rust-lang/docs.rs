//! Simple module to store files in database.
//!
//! cratesfyi is generating more than 5 million files, they are small and mostly html files.
//! They are using so many inodes and it is better to store them in database instead of
//! filesystem. This module is adding files into database and retrieving them.

use crate::error::Result;
use failure::err_msg;
use postgres::Connection;
use rustc_serialize::json::{Json, ToJson};
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn file_path(prefix: &str, name: &str) -> String {
    match prefix.is_empty() {
        true => name.to_owned(),
        false => format!("{}/{}", prefix, name),
    }
}

fn get_file_list_from_dir<P: AsRef<Path>>(
    path: P,
    prefix: &str,
    files: &mut Vec<String>,
) -> Result<()> {
    let path = path.as_ref();

    for file in path.read_dir()? {
        let file = file?;

        if file.file_type()?.is_file() {
            file.file_name()
                .to_str()
                .map(|name| files.push(file_path(prefix, name)));
        } else if file.file_type()?.is_dir() {
            file.file_name()
                .to_str()
                .map(|name| get_file_list_from_dir(file.path(), &file_path(prefix, name), files));
        }
    }

    Ok(())
}

pub fn get_file_list<P: AsRef<Path>>(path: P) -> Result<Vec<String>> {
    let path = path.as_ref();
    let mut files: Vec<String> = Vec::new();

    if !path.exists() {
        return Err(err_msg("File not found"));
    } else if path.is_file() {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| files.push(format!("{}", name)));
    } else if path.is_dir() {
        get_file_list_from_dir(path, "", &mut files)?;
    }

    Ok(files)
}

/// Adds files into database and returns list of files with their mime type in Json
pub fn add_path_into_database<P: AsRef<Path>>(
    conn: &Connection,
    prefix: &str,
    path: P,
) -> Result<Json> {
    use magic::{flags, Cookie};
    let cookie = Cookie::open(flags::MIME_TYPE)?;
    cookie.load::<&str>(&[])?;

    let trans = conn.transaction()?;

    let mut file_list_with_mimes: Vec<(String, String)> = Vec::new();

    for file_path_str in get_file_list(&path)? {
        let (path, content, mime) = {
            let path = Path::new(path.as_ref()).join(&file_path_str);
            // Some files have insufficient permissions (like .lock file created by cargo in
            // documentation directory). We are skipping this files.
            let mut file = match File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut content: Vec<u8> = Vec::new();
            file.read_to_end(&mut content)?;
            let mime = {
                let mime = cookie.buffer(&content)?;
                // css's are causing some problem in browsers
                // magic will return text/plain for css file types
                // convert them to text/css
                // do the same for javascript files
                if mime == "text/plain" {
                    if file_path_str.ends_with(".css") {
                        "text/css".to_owned()
                    } else if file_path_str.ends_with(".js") {
                        "application/javascript".to_owned()
                    } else {
                        mime.to_owned()
                    }
                } else {
                    mime.to_owned()
                }
            };

            file_list_with_mimes.push((mime.clone(), file_path_str.clone()));

            (file_path(prefix, &file_path_str), content, mime)
        };

        // check if file already exists in database
        let rows = conn.query("SELECT COUNT(*) FROM files WHERE path = $1", &[&path])?;

        if rows.get(0).get::<usize, i64>(0) == 0 {
            trans.query(
                "INSERT INTO files (path, mime, content) VALUES ($1, $2, $3)",
                &[&path, &mime, &content],
            )?;
        } else {
            trans.query(
                "UPDATE files SET mime = $2, content = $3, date_updated = NOW() \
                 WHERE path = $1",
                &[&path, &mime, &content],
            )?;
        }
    }

    trans.commit()?;

    file_list_to_json(file_list_with_mimes)
}

fn file_list_to_json(file_list: Vec<(String, String)>) -> Result<Json> {
    let mut file_list_json: Vec<Json> = Vec::new();

    for file in file_list {
        let mut v: Vec<String> = Vec::new();
        v.push(file.0.clone());
        v.push(file.1.clone());
        file_list_json.push(v.to_json());
    }

    Ok(file_list_json.to_json())
}

#[cfg(test)]
mod test {
    use super::super::connect_db;
    use super::{add_path_into_database, get_file_list};
    use std::env;

    #[test]
    fn test_get_file_list() {
        let _ = env_logger::try_init();

        let files = get_file_list(env::current_dir().unwrap());
        assert!(files.is_ok());
        assert!(files.unwrap().len() > 0);

        let files = get_file_list(env::current_dir().unwrap().join("Cargo.toml")).unwrap();
        assert_eq!(files[0], "Cargo.toml");
    }

    #[test]
    #[ignore]
    fn test_add_path_into_database() {
        let _ = env_logger::try_init();

        let conn = connect_db().unwrap();
        let res = add_path_into_database(&conn, "example", env::current_dir().unwrap().join("src"));
        assert!(res.is_ok());
    }
}
