use crate::{db::connect_db, error::Result};
use failure::err_msg;
use regex::Regex;
use time;

/// Fields we need use in cratesfyi
#[derive(Debug)]
struct GitHubFields {
    description: String,
    stars: i64,
    forks: i64,
    issues: i64,
    last_commit: time::Timespec,
}

/// Updates github fields in crates table
pub fn github_updater() -> Result<()> {
    let conn = connect_db()?;

    // TODO: This query assumes repository field in Cargo.toml is
    //       always the same across all versions of a crate
    for row in &conn.query(
        "SELECT DISTINCT ON (crates.name)
                                        crates.name,
                                        crates.id,
                                        releases.repository_url
                                 FROM crates
                                 INNER JOIN releases ON releases.crate_id = crates.id
                                 WHERE releases.repository_url ~ '^https*://github.com' AND
                                       (crates.github_last_update < NOW() - INTERVAL '1 day' OR
                                        crates.github_last_update IS NULL)
                                 ORDER BY crates.name, releases.release_time DESC",
        &[],
    )? {
        let crate_name: String = row.get(0);
        let crate_id: i32 = row.get(1);
        let repository_url: String = row.get(2);

        if let Err(err) = get_github_path(&repository_url[..])
            .ok_or_else(|| err_msg("Failed to get github path"))
            .and_then(|path| get_github_fields(&path[..]))
            .and_then(|fields| {
                conn.execute(
                    "UPDATE crates
                              SET github_description = $1,
                                  github_stars = $2, github_forks = $3,
                                  github_issues = $4, github_last_commit = $5,
                                  github_last_update = NOW() WHERE id = $6",
                    &[
                        &fields.description,
                        &(fields.stars as i32),
                        &(fields.forks as i32),
                        &(fields.issues as i32),
                        &(fields.last_commit),
                        &crate_id,
                    ],
                )
                .or_else(|e| Err(e.into()))
            })
        {
            debug!("Failed to update github fields of: {} {}", crate_name, err);
        }

        // sleep for rate limits
        use std::thread;
        use std::time::Duration;
        thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

fn get_github_fields(path: &str) -> Result<GitHubFields> {
    use rustc_serialize::json::Json;

    let body = {
        use reqwest::header::USER_AGENT;
        use reqwest::{Client, StatusCode};
        use std::env;
        use std::io::Read;

        let client = Client::new();
        let mut body = String::new();

        let mut resp = client
            .get(&format!("https://api.github.com/repos/{}", path)[..])
            .header(
                USER_AGENT,
                format!("cratesfyi/{}", env!("CARGO_PKG_VERSION")),
            )
            .basic_auth(
                env::var("CRATESFYI_GITHUB_USERNAME")
                    .ok()
                    .unwrap_or_default(),
                env::var("CRATESFYI_GITHUB_ACCESSTOKEN").ok(),
            )
            .send()?;

        if resp.status() != StatusCode::OK {
            return Err(err_msg("Failed to get github data"));
        }

        resp.read_to_string(&mut body)?;
        body
    };

    let json = Json::from_str(&body[..])?;
    let obj = json.as_object().unwrap();

    Ok(GitHubFields {
        description: obj
            .get("description")
            .and_then(|d| d.as_string())
            .unwrap_or("")
            .to_string(),
        stars: obj
            .get("stargazers_count")
            .and_then(|d| d.as_i64())
            .unwrap_or(0),
        forks: obj.get("forks_count").and_then(|d| d.as_i64()).unwrap_or(0),
        issues: obj.get("open_issues").and_then(|d| d.as_i64()).unwrap_or(0),
        last_commit: time::strptime(
            obj.get("pushed_at")
                .and_then(|d| d.as_string())
                .unwrap_or(""),
            "%Y-%m-%dT%H:%M:%S",
        )
        .unwrap_or_else(|_| time::now())
        .to_timespec(),
    })
}

fn get_github_path(url: &str) -> Option<String> {
    let re = Regex::new(r"https?://github\.com/([\w\._-]+)/([\w\._-]+)").unwrap();
    match re.captures(url) {
        Some(cap) => {
            let username = cap.get(1).unwrap().as_str();
            let reponame = cap.get(2).unwrap().as_str();
            
            let reponame = if reponame.ends_with(".git") {
                reponame.split(".git").next().unwrap()
            } else {
                reponame
            };

            Some(format!("{}/{}", username, reponame))
        }

        None => None,
    }
}

#[cfg(test)]
mod test {
    use super::{get_github_fields, get_github_path, github_updater};

    #[test]
    fn test_get_github_path() {
        assert_eq!(
            get_github_path("https://github.com/onur/cratesfyi"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("http://github.com/onur/cratesfyi"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/onur/cratesfyi.git"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/onur23cmD_M_R_L_/crates_fy-i"),
            Some("onur23cmD_M_R_L_/crates_fy-i".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/docopt/docopt.rs"),
            Some("docopt/docopt.rs".to_string())
        );
    }

    #[test]
    #[ignore]
    fn test_get_github_fields() {
        let _ = env_logger::try_init();
        let fields = get_github_fields("onur/cratesfyi");
        assert!(fields.is_ok());

        let fields = fields.unwrap();
        assert!(fields.description != "".to_string());
        assert!(fields.stars >= 0);
        assert!(fields.forks >= 0);
        assert!(fields.issues >= 0);

        use time;
        assert!(fields.last_commit <= time::now().to_timespec());
    }

    #[test]
    #[ignore]
    fn test_github_updater() {
        let _ = env_logger::try_init();
        assert!(github_updater().is_ok());
    }
}
