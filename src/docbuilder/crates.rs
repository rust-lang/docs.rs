use crate::error::Result;
use failure::err_msg;
use rustc_serialize::json::Json;
use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;

fn crates_from_file<F>(path: &PathBuf, func: &mut F) -> Result<()>
where
    F: FnMut(&str, &str) -> (),
{
    let reader = fs::File::open(path).map(|f| BufReader::new(f))?;

    let mut name = String::new();
    let mut versions = Vec::new();

    for line in reader.lines() {
        // some crates have invalid UTF-8 (nanny-sys-0.0.7)
        // skip them
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let data = match Json::from_str(line.trim()) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let obj = data
            .as_object()
            .ok_or_else(|| err_msg("Not a JSON object"))?;
        let crate_name = obj
            .get("name")
            .and_then(|n| n.as_string())
            .ok_or_else(|| err_msg("`name` not found in JSON object"))?;
        let vers = obj
            .get("vers")
            .and_then(|n| n.as_string())
            .ok_or_else(|| err_msg("`vers` not found in JSON object"))?;

        // Skip yanked crates
        if obj
            .get("yanked")
            .and_then(|n| n.as_boolean())
            .unwrap_or(false)
        {
            continue;
        }

        name.clear();
        name.push_str(crate_name);
        versions.push(format!("{}", vers));
    }

    if !name.is_empty() {
        versions.reverse();
        for version in versions {
            func(&name[..], &version[..]);
        }
    }

    Ok(())
}

pub fn crates_from_path<F>(path: &PathBuf, func: &mut F) -> Result<()>
where
    F: FnMut(&str, &str) -> (),
{
    if !path.is_dir() {
        return Err(err_msg("Not a directory"));
    }

    for file in path.read_dir()? {
        let file = file?;
        let path = file.path();
        // skip files under .git and config.json
        if path.to_str().unwrap().contains(".git") || path.file_name().unwrap() == "config.json" {
            continue;
        }

        if path.is_dir() {
            crates_from_path(&path, func)?;
        } else {
            crates_from_file(&path, func)?;
        }
    }

    Ok(())
}
