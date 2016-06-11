//! Simple module to store files in database.
//!
//! cratesfyi is generating more than 5 million files, they are small and mostly html files.
//! They are using so many inodes and it is better to store them in database instead of
//! filesystem. This module is adding files into database and retrieving them.


use std::path::Path;
use DocBuilderError;
use postgres::Connection;
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


fn get_file_list<P: AsRef<Path>>(path: P) -> Result<Vec<String>, DocBuilderError> {
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


/// Adds files into database
pub fn add_path_into_database<P: AsRef<Path>>(conn: &Connection,
                                              prefix: &str,
                                              path: P)
                                              -> Result<(), DocBuilderError> {
    use magic::{Cookie, flags};
    let cookie = try!(Cookie::open(flags::MIME_TYPE));
    // FIXME: This is linux specific but idk any alternative
    try!(cookie.load(&vec!["/usr/share/misc/magic.mgc"]));

    let trans = try!(conn.transaction());

    for file_path_str in try!(get_file_list(&path)) {
        let (content, mime) = {
            let path = Path::new(path.as_ref()).join(&file_path_str);
            let mut file = try!(File::open(path));
            let mut content: Vec<u8> = Vec::new();
            try!(file.read_to_end(&mut content));
            let mime = try!(cookie.buffer(&content));
            (content, mime)
        };

        try!(trans.query("INSERT INTO files (path, mime, content) VALUES ($1, $2, $3)",
                         &[&file_path(prefix, &file_path_str), &mime, &content]));
    }

    try!(trans.commit());

    Ok(())
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
        debug!("{:#?}", files);
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
        add_path_into_database(&conn, "example", env::current_dir().unwrap().join("src")).unwrap();
    }
}
