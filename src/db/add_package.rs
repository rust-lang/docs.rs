use crate::utils::source_path;
use crate::ChrootBuilderResult;
use crate::Metadata;
use regex::Regex;

use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::error::Result;
use cargo::core::dependency::Kind;
use cargo::core::{Package, TargetKind};
use failure::err_msg;
use postgres::Connection;
use reqwest::header::ACCEPT;
use reqwest::Client;
use rustc_serialize::json::{Json, ToJson};
use slug::slugify;

/// Adds a package into database.
///
/// Package must be built first.
pub fn add_package_into_database(
    conn: &Connection,
    pkg: &Package,
    res: &ChrootBuilderResult,
    files: Option<Json>,
    doc_targets: Vec<String>,
) -> Result<i32> {
    debug!("Adding package into database");
    let crate_id = initialize_package_in_database(&conn, &pkg)?;
    let dependencies = convert_dependencies(&pkg);
    let rustdoc = get_rustdoc(&pkg).unwrap_or(None);
    let readme = get_readme(&pkg).unwrap_or(None);
    let (release_time, yanked, downloads) = get_release_time_yanked_downloads(&pkg)?;
    let is_library = match pkg.targets()[0].kind() {
        &TargetKind::Lib(_) => true,
        _ => false,
    };
    let metadata = Metadata::from_package(pkg)?;

    let release_id: i32 = {
        let rows = conn.query(
            "SELECT id FROM releases WHERE crate_id = $1 AND version = $2",
            &[&crate_id, &format!("{}", pkg.manifest().version())],
        )?;

        if rows.len() == 0 {
            let rows = conn.query(
                "INSERT INTO releases (
                                            crate_id, version, release_time,
                                            dependencies, target_name, yanked, build_status,
                                            rustdoc_status, test_status, license, repository_url,
                                            homepage_url, description, description_long, readme,
                                            authors, keywords, have_examples, downloads, files,
                                            doc_targets, is_library, doc_rustc_version,
                                            documentation_url, default_target
                                        )
                                        VALUES ( $1,  $2,  $3,  $4, $5, $6,  $7, $8, $9, $10,
                                                 $11, $12, $13, $14, $15, $16, $17, $18, $19,
                                                 $20, $21, $22, $23, $24, $25
                                        )
                                        RETURNING id",
                &[
                    &crate_id,
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
                    &res.rustc_version,
                    &pkg.manifest().metadata().documentation,
                    &metadata.default_target,
                ],
            )?;
            // return id
            rows.get(0).get(0)
        } else {
            conn.query(
                "UPDATE releases
                             SET release_time = $3,
                                 dependencies = $4,
                                 target_name = $5,
                                 yanked = $6,
                                 build_status = $7,
                                 rustdoc_status = $8,
                                 test_status = $9,
                                 license = $10,
                                 repository_url = $11,
                                 homepage_url = $12,
                                 description = $13,
                                 description_long = $14,
                                 readme = $15,
                                 authors = $16,
                                 keywords = $17,
                                 have_examples = $18,
                                 downloads = $19,
                                 files = $20,
                                 doc_targets = $21,
                                 is_library = $22,
                                 doc_rustc_version = $23,
                                 documentation_url = $24,
                                 default_target = $25
                             WHERE crate_id = $1 AND version = $2",
                &[
                    &crate_id,
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
                    &res.rustc_version,
                    &pkg.manifest().metadata().documentation,
                    &metadata.default_target,
                ],
            )?;
            rows.get(0).get(0)
        }
    };

    add_keywords_into_database(&conn, &pkg, &release_id)?;
    add_authors_into_database(&conn, &pkg, &release_id)?;
    add_owners_into_database(&conn, &pkg, &crate_id)?;

    // Update versions
    {
        let mut versions: Json = conn
            .query("SELECT versions FROM crates WHERE id = $1", &[&crate_id])?
            .get(0)
            .get(0);
        if let Some(versions_array) = versions.as_array_mut() {
            let mut found = false;
            for version in versions_array.clone() {
                if &semver::Version::parse(version.as_string().unwrap()).unwrap()
                    == pkg.manifest().version()
                {
                    found = true;
                }
            }
            if !found {
                versions_array.push(format!("{}", &pkg.manifest().version()).to_json());
            }
        }
        let _ = conn.query(
            "UPDATE crates SET versions = $1 WHERE id = $2",
            &[&versions, &crate_id],
        );
    }

    Ok(release_id)
}

/// Adds a build into database
pub fn add_build_into_database(
    conn: &Connection,
    release_id: &i32,
    res: &ChrootBuilderResult,
) -> Result<i32> {
    debug!("Adding build into database");
    let rows = conn.query(
        "INSERT INTO builds (rid, rustc_version,
                                                    cratesfyi_version,
                                                    build_status, output)
                                VALUES ($1, $2, $3, $4, $5)
                                RETURNING id",
        &[
            release_id,
            &res.rustc_version,
            &res.cratesfyi_version,
            &res.build_success,
            &res.output,
        ],
    )?;
    Ok(rows.get(0).get(0))
}

fn initialize_package_in_database(conn: &Connection, pkg: &Package) -> Result<i32> {
    let mut rows = conn.query(
        "SELECT id FROM crates WHERE name = $1",
        &[&pkg.manifest().name().as_str()],
    )?;
    // insert crate into database if it is not exists
    if rows.len() == 0 {
        rows = conn.query(
            "INSERT INTO crates (name) VALUES ($1) RETURNING id",
            &[&pkg.manifest().name().as_str()],
        )?;
    }
    Ok(rows.get(0).get(0))
}

/// Convert dependencies into Vec<(String, String, String)>
fn convert_dependencies(pkg: &Package) -> Vec<(String, String, String)> {
    let mut dependencies: Vec<(String, String, String)> = Vec::new();
    for dependency in pkg.manifest().dependencies() {
        let name = dependency.package_name().to_string();
        let version = format!("{}", dependency.version_req());
        let kind = match dependency.kind() {
            Kind::Normal => "normal",
            Kind::Development => "dev",
            Kind::Build => "build",
        };
        dependencies.push((name, version, kind.to_string()));
    }
    dependencies
}

/// Reads readme if there is any read defined in Cargo.toml of a Package
fn get_readme(pkg: &Package) -> Result<Option<String>> {
    let readme_path = PathBuf::from(source_path(&pkg).ok_or_else(|| err_msg("File not found"))?)
        .join(
            pkg.manifest()
                .metadata()
                .readme
                .clone()
                .unwrap_or("README.md".to_owned()),
        );

    if !readme_path.exists() {
        return Ok(None);
    }

    let mut reader = fs::File::open(readme_path).map(|f| BufReader::new(f))?;
    let mut readme = String::new();
    reader.read_to_string(&mut readme)?;

    if readme.is_empty() {
        Ok(None)
    } else if readme.len() > 51200 {
        Ok(Some(format!(
            "(Readme ignored due to being too long. ({} > 51200))",
            readme.len()
        )))
    } else {
        Ok(Some(readme))
    }
}

fn get_rustdoc(pkg: &Package) -> Result<Option<String>> {
    if let Some(src_path) = pkg.manifest().targets()[0].src_path().path() {
        if src_path.is_absolute() {
            read_rust_doc(src_path)
        } else {
            let mut path =
                PathBuf::from(source_path(&pkg).ok_or_else(|| err_msg("File not found"))?);
            path.push(src_path);
            read_rust_doc(path.as_path())
        }
    } else {
        // FIXME: should we care about metabuild targets?
        Ok(None)
    }
}

/// Reads rustdoc from library
fn read_rust_doc(file_path: &Path) -> Result<Option<String>> {
    let reader = fs::File::open(file_path).map(|f| BufReader::new(f))?;
    let mut rustdoc = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.starts_with("//!") {
            // some lines may or may not have a space between the `//!` and the start of the text
            let line = line.trim_start_matches("//!").trim_start();
            if !line.is_empty() {
                rustdoc.push_str(line);
            }
            rustdoc.push('\n');
        }
    }

    if rustdoc.is_empty() {
        Ok(None)
    } else if rustdoc.len() > 51200 {
        Ok(Some(format!(
            "(Library doc comment ignored due to being too long. ({} > 51200))",
            rustdoc.len()
        )))
    } else {
        Ok(Some(rustdoc))
    }
}

/// Get release_time, yanked and downloads from crates.io
fn get_release_time_yanked_downloads(
    pkg: &Package,
) -> Result<(Option<time::Timespec>, Option<bool>, Option<i32>)> {
    let url = format!(
        "https://crates.io/api/v1/crates/{}/versions",
        pkg.manifest().name()
    );
    // FIXME: There is probably better way to do this
    //        and so many unwraps...
    let client = Client::new();
    let mut res = client
        .get(&url[..])
        .header(ACCEPT, "application/json")
        .send()?;
    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();
    let json = Json::from_str(&body[..]).unwrap();
    let versions = json
        .as_object()
        .and_then(|o| o.get("versions"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| err_msg("Not a JSON object"))?;

    let (mut release_time, mut yanked, mut downloads) = (None, None, None);

    for version in versions {
        let version = version
            .as_object()
            .ok_or_else(|| err_msg("Not a JSON object"))?;
        let version_num = version
            .get("num")
            .and_then(|v| v.as_string())
            .ok_or_else(|| err_msg("Not a JSON object"))?;

        if &semver::Version::parse(version_num).unwrap() == pkg.manifest().version() {
            let release_time_raw = version
                .get("created_at")
                .and_then(|c| c.as_string())
                .ok_or_else(|| err_msg("Not a JSON object"))?;
            release_time = Some(
                time::strptime(release_time_raw, "%Y-%m-%dT%H:%M:%S")
                    .unwrap()
                    .to_timespec(),
            );

            yanked = Some(
                version
                    .get("yanked")
                    .and_then(|c| c.as_boolean())
                    .ok_or_else(|| err_msg("Not a JSON object"))?,
            );

            downloads = Some(
                version
                    .get("downloads")
                    .and_then(|c| c.as_i64())
                    .ok_or_else(|| err_msg("Not a JSON object"))? as i32,
            );

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
            let rows = conn.query("SELECT id FROM keywords WHERE slug = $1", &[&slug])?;
            if rows.len() > 0 {
                rows.get(0).get(0)
            } else {
                conn.query(
                    "INSERT INTO keywords (name, slug) VALUES ($1, $2) RETURNING id",
                    &[&keyword, &slug],
                )?
                .get(0)
                .get(0)
            }
        };
        // add releationship
        let _ = conn.query(
            "INSERT INTO keyword_rels (rid, kid) VALUES ($1, $2)",
            &[release_id, &keyword_id],
        );
    }

    Ok(())
}

/// Adds authors into database
fn add_authors_into_database(conn: &Connection, pkg: &Package, release_id: &i32) -> Result<()> {
    let author_capture_re = Regex::new("^([^><]+)<*(.*?)>*$").unwrap();
    for author in &pkg.manifest().metadata().authors {
        if let Some(author_captures) = author_capture_re.captures(&author[..]) {
            let author = author_captures
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("")
                .trim();
            let email = author_captures
                .get(2)
                .map(|m| m.as_str())
                .unwrap_or("")
                .trim();
            let slug = slugify(&author);

            let author_id: i32 = {
                let rows = conn.query("SELECT id FROM authors WHERE slug = $1", &[&slug])?;
                if rows.len() > 0 {
                    rows.get(0).get(0)
                } else {
                    conn.query(
                        "INSERT INTO authors (name, email, slug) VALUES ($1, $2, $3)
                                     RETURNING id",
                        &[&author, &email, &slug],
                    )?
                    .get(0)
                    .get(0)
                }
            };

            // add relationship
            let _ = conn.query(
                "INSERT INTO author_rels (rid, aid) VALUES ($1, $2)",
                &[release_id, &author_id],
            );
        }
    }

    Ok(())
}

/// Adds owners into database
fn add_owners_into_database(conn: &Connection, pkg: &Package, crate_id: &i32) -> Result<()> {
    // owners available in: https://crates.io/api/v1/crates/rand/owners
    let owners_url = format!(
        "https://crates.io/api/v1/crates/{}/owners",
        &pkg.manifest().name()
    );
    let client = Client::new();
    let mut res = client
        .get(&owners_url[..])
        .header(ACCEPT, "application/json")
        .send()?;
    // FIXME: There is probably better way to do this
    //        and so many unwraps...
    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();
    let json = Json::from_str(&body[..])?;

    if let Some(owners) = json
        .as_object()
        .and_then(|j| j.get("users"))
        .and_then(|j| j.as_array())
    {
        for owner in owners {
            // FIXME: I know there is a better way to do this
            let avatar = owner
                .as_object()
                .and_then(|o| o.get("avatar"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let email = owner
                .as_object()
                .and_then(|o| o.get("email"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let login = owner
                .as_object()
                .and_then(|o| o.get("login"))
                .and_then(|o| o.as_string())
                .unwrap_or("");
            let name = owner
                .as_object()
                .and_then(|o| o.get("name"))
                .and_then(|o| o.as_string())
                .unwrap_or("");

            if login.is_empty() {
                continue;
            }

            let owner_id: i32 = {
                let rows = conn.query("SELECT id FROM owners WHERE login = $1", &[&login])?;
                if rows.len() > 0 {
                    rows.get(0).get(0)
                } else {
                    conn.query(
                        "INSERT INTO owners (login, avatar, name, email)
                                     VALUES ($1, $2, $3, $4)
                                     RETURNING id",
                        &[&login, &avatar, &name, &email],
                    )?
                    .get(0)
                    .get(0)
                }
            };

            // add relationship
            let _ = conn.query(
                "INSERT INTO owner_rels (cid, oid) VALUES ($1, $2)",
                &[crate_id, &owner_id],
            );
        }
    }
    Ok(())
}
