//! Simple module to store files in database.
//!
//! cratesfyi is generating more than 5 million files, they are small and mostly html files.
//! They are using so many inodes and it is better to store them in database instead of
//! filesystem. This module is adding files into database and retrieving them.


use std::path::Path;
use DocBuilderError;
use postgres::Connection;
use rustc_serialize::json::{Json, ToJson};
use std::fs::File;
use std::io::Read;



fn file_path(prefix: &str, name: &str) -> String {
    match prefix.is_empty() {
        true => name.to_owned(),
        false => format!("{}/{}", prefix, name),
    }
}


fn get_file_list_from_dir<P: AsRef<Path>>(path: P,
                                          prefix: &str,
                                          files: &mut Vec<String>)
                                          -> Result<(), DocBuilderError> {
    let path = path.as_ref();

    for file in try!(path.read_dir()) {
        let file = try!(file);

        if try!(file.file_type()).is_file() {
            file.file_name().to_str().map(|name| files.push(file_path(prefix, name)));
        } else if try!(file.file_type()).is_dir() {
            file.file_name()
                .to_str()
                .map(|name| get_file_list_from_dir(file.path(), &file_path(prefix, name), files));
        }
    }

    Ok(())
}


pub fn get_file_list<P: AsRef<Path>>(path: P) -> Result<Vec<String>, DocBuilderError> {
    let path = path.as_ref();
    let mut files: Vec<String> = Vec::new();

    if !path.exists() {
        return Err(DocBuilderError::FileNotFound);
    } else if path.is_file() {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| files.push(format!("{}", name)));
    } else if path.is_dir() {
        try!(get_file_list_from_dir(path, "", &mut files));
    }

    Ok(files)
}


/// Adds files into database and returns list of files with their mime type in Json
pub fn add_path_into_database<P: AsRef<Path>>(conn: &Connection,
                                              prefix: &str,
                                              path: P)
                                              -> Result<Json, DocBuilderError> {
    use magic::{Cookie, flags};
    let cookie = try!(Cookie::open(flags::MIME_TYPE));
    // FIXME: This is linux specific but idk any alternative
    try!(cookie.load(&vec!["/usr/share/misc/magic.mgc"]));

    let trans = try!(conn.transaction());

    let mut file_list_with_mimes: Vec<(String, String)> = Vec::new();

    for file_path_str in try!(get_file_list(&path)) {
        let (path, content, mime) = {
            let path = Path::new(path.as_ref()).join(&file_path_str);
            // Some files have insufficient permissions (like .lock file created by cargo in
            // documentation directory). We are skipping this files.
            let mut file = match File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut content: Vec<u8> = Vec::new();
            try!(file.read_to_end(&mut content));
            let mime = try!(cookie.buffer(&content));

            file_list_with_mimes.push((mime.clone(), file_path_str.clone()));

            (file_path(prefix, &file_path_str), content, mime)
        };

        // check if file already exists in database
        let rows = try!(conn.query("SELECT COUNT(*) FROM files WHERE path = $1", &[&path]));

        if rows.get(0).get::<usize, i64>(0) == 0 {
            try!(trans.query("INSERT INTO files (path, mime, content) VALUES ($1, $2, $3)",
                             &[&path, &mime, &content]));
        } else {
            try!(trans.query("UPDATE files SET mime = $2, content = $3, date_updated = NOW() \
                              WHERE path = $1",
                             &[&path, &mime, &content]));
        }
    }

    try!(trans.commit());

    file_list_to_json(file_list_with_mimes)
}



fn file_list_to_json(file_list: Vec<(String, String)>) -> Result<Json, DocBuilderError> {

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
    extern crate env_logger;
    use std::env;
    use super::{get_file_list, add_path_into_database};
    use super::super::connect_db;

    #[test]
    fn test_get_file_list() {
        let _ = env_logger::init();

        let files = get_file_list(env::current_dir().unwrap());
        assert!(files.is_ok());
        assert!(files.unwrap().len() > 0);

        let files = get_file_list(env::current_dir().unwrap().join("Cargo.toml")).unwrap();
        assert_eq!(files[0], "Cargo.toml");
    }

    #[test]
    #[ignore]
    fn test_add_path_into_database() {
        let _ = env_logger::init();

        let conn = connect_db().unwrap();
        let res = add_path_into_database(&conn, "example", env::current_dir().unwrap().join("src"));
        assert!(res.is_ok());
    }
}
