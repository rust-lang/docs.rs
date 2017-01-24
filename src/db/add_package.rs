
use ChrootBuilderResult;
use utils::source_path;
use regex::Regex;

use std::io::prelude::*;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::fs;

use cargo::core::{Package, TargetKind};
use rustc_serialize::json::{Json, ToJson};
use slug::slugify;
use reqwest::Client;
use reqwest::header::{Accept, qitem};
use semver;
use postgres::Connection;
use time;

use errors::*;



/// Adds a package into database.
///
/// Package must be built first.
pub fn add_package_into_database(conn: &Connection,
                                 pkg: &Package,
                                 res: &ChrootBuilderResult,
                                 files: Option<Json>,
                                 doc_targets: Vec<String>)
                                 -> Result<i32> {
    debug!("Adding package into database");
    let crate_id = try!(initialize_package_in_database(&conn, &pkg));
    let dependencies = convert_dependencies(&pkg);
    let rustdoc = get_rustdoc(&pkg).unwrap_or(None);
    let readme = get_readme(&pkg).unwrap_or(None);
    let (release_time, yanked, downloads) = try!(get_release_time_yanked_downloads(&pkg));
    let is_library = match pkg.targets()[0].kind() {
        &TargetKind::Lib(_) => true,
        _ => false,
    };

    let release_id: i32 = {
        let rows = try!(conn.query("SELECT id FROM releases WHERE crate_id = $1 AND version = $2",
                                   &[&crate_id, &format!("{}", pkg.manifest().version())]));

        if rows.len() == 0 {
            let rows = try!(conn.query("INSERT INTO releases (
                                            crate_id, version, release_time, \
                                            dependencies, target_name, yanked, build_status, \
                                            rustdoc_status, test_status, license, repository_url, \
                                            homepage_url, description, description_long, readme, \
                                            authors, keywords, have_examples, downloads, files, \
                                            doc_targets, is_library, doc_rustc_version \
                                        ) \
                                        VALUES ( $1,  $2,  $3,  $4, $5, $6,  $7, $8, $9, $10, \
                                                 $11, $12, $13, $14, $15, $16, $17, $18, $19, \
                                                 $20, $21, $22, $23 \
                                        ) \
                                        RETURNING id",
                                       &[&crate_id,
                                         &format!("{}", pkg.manifest().version()),
                                         &release_time,
                                         &dependencies.to_json(),
                                         &pkg.targets()[0].name().replace("-", "_"),
                                         &yanked,
                                         &res.build_success,
                                         &res.have_doc,
                                         &false, // TODO: Add test status somehow
                                         &pkg.manifest().metadata().license,
                                         &pkg.manifest().metadata().repository,
                                         &pkg.manifest().metadata().homepage,
                                         &pkg.manifest().metadata().description,
                                         &rustdoc,
                                         &readme,
                                         &pkg.manifest().metadata().authors.to_json(),
                                         &pkg.manifest().metadata().keywords.to_json(),
                                         &res.have_examples,
                                         &downloads,
                                         &files,
                                         &doc_targets.to_json(),
                                         &is_library,
                                         &res.rustc_version]));
            // return id
            rows.get(0).get(0)

        } else {
            try!(conn.query("UPDATE releases SET release_time = $3, dependencies = $4, \
                             target_name = $5, yanked = $6, build_status = $7, rustdoc_status = \
                             $8, test_status = $9, license = $10, repository_url = $11, \
                             homepage_url = $12, description = $13, description_long = $14, \
                             readme = $15, authors = $16, keywords = $17, have_examples = $18, \
                             downloads = $19, files = $20, doc_targets = $21, is_library = $22, \
                             doc_rustc_version = $23 \
                             WHERE crate_id = $1 AND version = $2",
                            &[&crate_id,
                              &format!("{}", pkg.manifest().version()),
                              &release_time,
                              &dependencies.to_json(),
                              &pkg.targets()[0].name().replace("-", "_"),
                              &yanked,
                              &res.build_success,
                              &res.have_doc,
                              &false, // TODO: Add test status somehow
                              &pkg.manifest().metadata().license,
                              &pkg.manifest().metadata().repository,
                              &pkg.manifest().metadata().homepage,
                              &pkg.manifest().metadata().description,
                              &rustdoc,
                              &readme,
                              &pkg.manifest().metadata().authors.to_json(),
                              &pkg.manifest().metadata().keywords.to_json(),
                              &res.have_examples,
                              &downloads,
                              &files,
                              &doc_targets.to_json(),
                              &is_library,
                              &res.rustc_version]));
            rows.get(0).get(0)
        }
    };


    try!(add_keywords_into_database(&conn, &pkg, &release_id));
    try!(add_authors_into_database(&conn, &pkg, &release_id));
    try!(add_owners_into_database(&conn, &pkg, &crate_id));


    // Update versions
    {
        let mut versions: Json = try!(conn.query("SELECT versions FROM crates WHERE id = $1",
                                                 &[&crate_id]))
            .get(0)
            .get(0);
        if let Some(versions_array) = versions.as_array_mut() {
            let mut found = false;
            for version in versions_array.clone() {
                if &semver::Version::parse(version.as_string().unwrap()).unwrap() ==
                   pkg.manifest().version() {
                    found = true;
                }
            }
            if !found {
                versions_array.push(format!("{}", &pkg.manifest().version()).to_json());
            }
        }
        let _ = conn.query("UPDATE crates SET versions = $1 WHERE id = $2",
                           &[&versions, &crate_id]);
    }

    Ok(release_id)
}


/// Adds a build into database
pub fn add_build_into_database(conn: &Connection,
                               release_id: &i32,
                               res: &ChrootBuilderResult)
                               -> Result<i32> {
    debug!("Adding build into database");
    let rows = try!(conn.query("INSERT INTO builds (rid, rustc_version, cratesfyi_version, \
                                build_status, output)
                                VALUES \
                                ($1, $2, $3, $4, $5) RETURNING id",
                               &[release_id,
                                 &res.rustc_version,
                                 &res.cratesfyi_version,
                                 &res.build_success,
                                 &res.output]));
    Ok(rows.get(0).get(0))
}


fn initialize_package_in_database(conn: &Connection, pkg: &Package) -> Result<i32> {
    let mut rows = try!(conn.query("SELECT id FROM crates WHERE name = $1",
                                   &[&pkg.manifest().name()]));
    // insert crate into database if it is not exists
    if rows.len() == 0 {
        rows = try!(conn.query("INSERT INTO crates (name) VALUES ($1) RETURNING id",
                               &[&pkg.manifest().name()]));
    }
    Ok(rows.get(0).get(0))
}



/// Convert dependencies into Vec<(String, String)>
fn convert_dependencies(pkg: &Package) -> Vec<(String, String)> {
    let mut dependencies: Vec<(String, String)> = Vec::new();
    for dependency in pkg.manifest().dependencies() {
        let name = dependency.name().to_string();
        let version = format!("{}", dependency.version_req());
        dependencies.push((name, version));
    }
    dependencies
}


/// Reads readme if there is any read defined in Cargo.toml of a Package
fn get_readme(pkg: &Package) -> Result<Option<String>> {
    let readme_path = PathBuf::from(try!(source_path(&pkg).ok_or("File not found")))
        .join(pkg.manifest().metadata().readme.clone().unwrap_or("README.md".to_owned()));

    if !readme_path.exists() {
        return Ok(None);
    }

    let mut reader = try!(fs::File::open(readme_path).map(|f| BufReader::new(f)));
    let mut readme = String::new();
    try!(reader.read_to_string(&mut readme));
    Ok(Some(readme))
}


fn get_rustdoc(pkg: &Package) -> Result<Option<String>> {
    if pkg.manifest().targets()[0].src_path().is_absolute() {
        read_rust_doc(pkg.manifest().targets()[0].src_path())
    } else {
        let mut path = PathBuf::from(try!(source_path(&pkg).ok_or("File not found")));
        path.push(pkg.manifest().targets()[0].src_path());
        read_rust_doc(path.as_path())
    }
}


/// Reads rustdoc from library
fn read_rust_doc(file_path: &Path) -> Result<Option<String>> {
    let reader = try!(fs::File::open(file_path).map(|f| BufReader::new(f)));
    let mut rustdoc = String::new();

    for line in reader.lines() {
        let line = try!(line);
        if line.starts_with("//!") {
            if line.len() > 3 {
                rustdoc.push_str(line.split_at(4).1);
            }
            rustdoc.push('\n');
        }
    }

    if rustdoc.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rustdoc))
    }
}



/// Get release_time, yanked and downloads from crates.io
fn get_release_time_yanked_downloads
    (pkg: &Package)
     -> Result<(Option<time::Timespec>, Option<bool>, Option<i32>)> {
    let url = format!("https://crates.io/api/v1/crates/{}/versions",
                      pkg.manifest().name());
    // FIXME: There is probably better way to do this
    //        and so many unwraps...
    let client = try!(Client::new());
    let mut res = try!(client.get(&url[..])
        .header(Accept(vec![qitem("application/json".parse().unwrap())]))
        .send());
    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();
    let json = Json::from_str(&body[..]).unwrap();
    let versions = try!(json.as_object()
        .and_then(|o| o.get("versions"))
        .and_then(|v| v.as_array())
        .ok_or("Not a JSON object"));

    let (mut release_time, mut yanked, mut downloads) = (None, None, None);

    for version in versions {
        let version = try!(version.as_object().ok_or("Not a JSON object"));
        let version_num = try!(version.get("num")
            .and_then(|v| v.as_string())
            .ok_or("Not a JSON object"));

        if &semver::Version::parse(version_num).unwrap() == pkg.manifest().version() {
            let release_time_raw = try!(version.get("created_at")
                .and_then(|c| c.as_string())
                .ok_or("Not a JSON object"));
            release_time = Some(time::strptime(release_time_raw, "%Y-%m-%dT%H:%M:%S")
                .unwrap()
                .to_timespec());

            yanked = Some(try!(version.get("yanked")
                .and_then(|c| c.as_boolean())
                .ok_or("Not a JSON object")));

            downloads = Some(try!(version.get("downloads")
                .and_then(|c| c.as_i64())
                .ok_or("Not a JSON object")) as i32);

            break;
        }
    }

    Ok((release_time, yanked, downloads))
}


/// Adds keywords into database
fn add_keywords_into_database(conn: &Connection, pkg: &Package, release_id: &i32) -> Result<()> {
    for keyword in &pkg.manifest().metadata().keywords {
        let slug = slugify(&keyword);
        let keyword_id: i32 = {
            let rows = try!(conn.query("SELECT id FROM keywords WHERE slug = $1", &[&slug]));
            if rows.len() > 0 {
                rows.get(0).get(0)
            } else {
                try!(conn.query("INSERT INTO keywords (name, slug) VALUES ($1, $2) RETURNING id",
                                &[&keyword, &slug]))
                    .get(0)
                    .get(0)
            }
        };
        // add releationship
        let _ = conn.query("INSERT INTO keyword_rels (rid, kid) VALUES ($1, $2)",
                           &[release_id, &keyword_id]);
    }

    Ok(())
}



/// Adds authors into database
fn add_authors_into_database(conn: &Connection, pkg: &Package, release_id: &i32) -> Result<()> {

    let author_capture_re = Regex::new("^([^><]+)<*(.*?)>*$").unwrap();
    for author in &pkg.manifest().metadata().authors {
        if let Some(author_captures) = author_capture_re.captures(&author[..]) {
            let author = author_captures.at(1).unwrap_or("").trim();
            let email = author_captures.at(2).unwrap_or("").trim();
            let slug = slugify(&author);

            let author_id: i32 = {
                let rows = try!(conn.query("SELECT id FROM authors WHERE slug = $1", &[&slug]));
                if rows.len() > 0 {
                    rows.get(0).get(0)
                } else {
                    try!(conn.query("INSERT INTO authors (name, email, slug) VALUES ($1, $2, $3) \
                                     RETURNING id",
                                    &[&author, &email, &slug]))
                        .get(0)
                        .get(0)
                }
            };

            // add relationship
            let _ = conn.query("INSERT INTO author_rels (rid, aid) VALUES ($1, $2)",
                               &[release_id, &author_id]);
        }
    }

    Ok(())
}



/// Adds owners into database
fn add_owners_into_database(conn: &Connection, pkg: &Package, crate_id: &i32) -> Result<()> {
    // owners available in: https://crates.io/api/v1/crates/rand/owners
    let owners_url = format!("https://crates.io/api/v1/crates/{}/owners",
                             &pkg.manifest().name());
    let client = try!(Client::new());
    let mut res = try!(client.get(&owners_url[..])
        .header(Accept(vec![qitem("application/json".parse().unwrap())]))
        .send());
    // FIXME: There is probably better way to do this
    //        and so many unwraps...
    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();
    let json = try!(Json::from_str(&body[..]));

    if let Some(owners) = json.as_object()
        .and_then(|j| j.get("users"))
        .and_then(|j| j.as_array()) {
        for owner in owners {
            // FIXME: I know there is a better way to do this
            let avatar = owner.as_object()
                .and_then(|o| o.get("avatar"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let email = owner.as_object()
                .and_then(|o| o.get("email"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let login = owner.as_object()
                .and_then(|o| o.get("login"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let name = owner.as_object()
                .and_then(|o| o.get("name"))
                .and_then(|o| o.as_string())
                .unwrap_or("");

            if login.is_empty() {
                continue;
            }

            let owner_id: i32 = {
                let rows = try!(conn.query("SELECT id FROM owners WHERE login = $1", &[&login]));
                if rows.len() > 0 {
                    rows.get(0).get(0)
                } else {
                    try!(conn.query("INSERT INTO owners (login, avatar, name, email) VALUES ($1, \
                                     $2, $3, $4) RETURNING id",
                                    &[&login, &avatar, &name, &email]))
                        .get(0)
                        .get(0)
                }
            };

            // add relationship
            let _ = conn.query("INSERT INTO owner_rels (cid, oid) VALUES ($1, $2)",
                               &[crate_id, &owner_id]);
        }

    }
    Ok(())
}
