use crc32fast::Hasher;
use std::{
    fs,
    io::{self, Read as _},
    path::Path,
};

pub fn crc32_for_path(path: impl AsRef<Path>) -> Result<[u8; 4], io::Error> {
    let path = path.as_ref();

    let mut file = fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = [0; 256 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finalize().to_be_bytes())
}
