use std::io::Read;

use crate::{error::Result, utils::MetadataPackage};

use failure::err_msg;
use reqwest::{header::ACCEPT, Client};
use rustc_serialize::json::Json;
use time::Timespec;

pub(crate) struct RegistryCrateData {
    pub(crate) release_time: Timespec,
    pub(crate) yanked: bool,
    pub(crate) downloads: i32,
    pub(crate) owners: Vec<CrateOwner>,
}

pub(crate) struct CrateOwner {
    pub(crate) avatar: String,
    pub(crate) email: String,
    pub(crate) login: String,
    pub(crate) name: String,
}

impl RegistryCrateData {
    pub(crate) fn get_from_network(pkg: &MetadataPackage) -> Result<Self> {
        let (release_time, yanked, downloads) = get_release_time_yanked_downloads(pkg)?;
        let owners = get_owners(pkg)?;

        Ok(Self {
            release_time,
            yanked,
            downloads,
            owners,
        })
    }
}

/// Get release_time, yanked and downloads from the registry's API
fn get_release_time_yanked_downloads(pkg: &MetadataPackage) -> Result<(time::Timespec, bool, i32)> {
    let url = format!("https://crates.io/api/v1/crates/{}/versions", pkg.name);
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

        if semver::Version::parse(version_num).unwrap().to_string() == pkg.version {
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

    Ok((
        release_time.unwrap_or_else(time::get_time),
        yanked.unwrap_or(false),
        downloads.unwrap_or(0),
    ))
}

/// Fetch owners from the registry's API
fn get_owners(pkg: &MetadataPackage) -> Result<Vec<CrateOwner>> {
    // owners available in: https://crates.io/api/v1/crates/rand/owners
    let owners_url = format!("https://crates.io/api/v1/crates/{}/owners", pkg.name);
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

    let mut result = Vec::new();
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

            result.push(CrateOwner {
                avatar: avatar.to_string(),
                email: email.to_string(),
                login: login.to_string(),
                name: name.to_string(),
            });
        }
    }

    Ok(result)
}
