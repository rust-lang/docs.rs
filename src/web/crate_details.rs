use super::{markdown, match_version, MatchSemver, MetaData};
use crate::utils::{get_correct_docsrs_style_file, report_error};
use crate::web::axum_cached_redirect;
use crate::web::rustdoc::RustdocHtmlParams;
use crate::{
    impl_axum_webpage,
    storage::PathNotFoundError,
    web::{
        cache::CachePolicy,
        encode_url_path,
        error::{AxumNope, AxumResult},
        extractors::DbConnection,
    },
    AsyncStorage,
};
use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Extension, Path},
    response::{IntoResponse, Response as AxumResponse},
};
use chrono::{DateTime, Utc};
use futures_util::stream::TryStreamExt;
use log::warn;
use serde::Deserialize;
use serde::{ser::Serializer, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tracing::{instrument, trace};

// TODO: Add target name and versions

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CrateDetails {
    name: String,
    version: String,
    description: Option<String>,
    owners: Vec<(String, String)>,
    dependencies: Option<Value>,
    #[serde(serialize_with = "optional_markdown")]
    readme: Option<String>,
    #[serde(serialize_with = "optional_markdown")]
    rustdoc: Option<String>, // this is description_long in database
    release_time: DateTime<Utc>,
    build_status: bool,
    pub latest_build_id: Option<i32>,
    last_successful_build: Option<String>,
    pub rustdoc_status: bool,
    pub archive_storage: bool,
    repository_url: Option<String>,
    homepage_url: Option<String>,
    keywords: Option<Value>,
    have_examples: bool, // need to check this manually
    pub target_name: String,
    releases: Vec<Release>,
    repository_metadata: Option<RepositoryMetadata>,
    pub(crate) metadata: MetaData,
    is_library: bool,
    license: Option<String>,
    pub(crate) documentation_url: Option<String>,
    total_items: Option<i32>,
    documented_items: Option<i32>,
    total_items_needing_examples: Option<i32>,
    items_with_examples: Option<i32>,
    /// Database id for this crate
    pub(crate) crate_id: i32,
    /// Database id for this release
    pub(crate) release_id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RepositoryMetadata {
    stars: i32,
    forks: i32,
    issues: i32,
    name: Option<String>,
}

fn optional_markdown<S>(markdown: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    markdown
        .as_ref()
        .map(|markdown| markdown::render(markdown))
        .serialize(serializer)
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct Release {
    pub id: i32,
    pub version: semver::Version,
    pub build_status: bool,
    pub yanked: bool,
    pub is_library: bool,
    pub rustdoc_status: bool,
    pub target_name: String,
}

impl CrateDetails {
    pub async fn new(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &str,
        version_or_latest: &str,
    ) -> Result<Option<CrateDetails>, anyhow::Error> {
        let krate = match sqlx::query!(
            r#"SELECT
                crates.id AS crate_id,
                releases.id AS release_id,
                crates.name,
                releases.version,
                releases.description,
                releases.dependencies,
                releases.readme,
                releases.description_long,
                releases.release_time,
                releases.build_status,
                (
                    SELECT id
                    FROM builds
                    WHERE
                        builds.rid = releases.id AND
                        builds.build_status = TRUE
                    ORDER BY build_time DESC
                    LIMIT 1
                ) AS latest_build_id,
                releases.rustdoc_status,
                releases.archive_storage,
                releases.repository_url,
                releases.homepage_url,
                releases.keywords,
                releases.have_examples,
                releases.target_name,
                repositories.host as "repo_host?",
                repositories.stars as "repo_stars?",
                repositories.forks as "repo_forks?",
                repositories.issues as "repo_issues?",
                repositories.name as "repo_name?",
                releases.is_library,
                releases.yanked,
                releases.doc_targets,
                releases.license,
                releases.documentation_url,
                releases.default_target,
                releases.doc_rustc_version,
                doc_coverage.total_items,
                doc_coverage.documented_items,
                doc_coverage.total_items_needing_examples,
                doc_coverage.items_with_examples
            FROM releases
            INNER JOIN crates ON releases.crate_id = crates.id
            LEFT JOIN doc_coverage ON doc_coverage.release_id = releases.id
            LEFT JOIN repositories ON releases.repository_id = repositories.id
            WHERE crates.name = $1 AND releases.version = $2;"#,
            name,
            version
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            Some(row) => row,
            None => return Ok(None),
        };

        // get releases, sorted by semver
        let releases = releases_for_crate(conn, krate.crate_id).await?;

        let repository_metadata = krate.repo_host.map(|_| RepositoryMetadata {
            issues: krate.repo_issues.unwrap(),
            stars: krate.repo_stars.unwrap(),
            forks: krate.repo_forks.unwrap(),
            name: krate.repo_name,
        });

        let metadata = MetaData {
            name: krate.name.clone(),
            version: krate.version.clone(),
            version_or_latest: version_or_latest.to_string(),
            description: krate.description.clone(),
            rustdoc_status: krate.rustdoc_status,
            target_name: Some(krate.target_name.clone()),
            default_target: krate.default_target,
            doc_targets: MetaData::parse_doc_targets(krate.doc_targets),
            yanked: krate.yanked,
            rustdoc_css_file: get_correct_docsrs_style_file(&krate.doc_rustc_version)?,
        };

        let mut crate_details = CrateDetails {
            name: krate.name,
            version: krate.version,
            description: krate.description,
            owners: Vec::new(),
            dependencies: krate.dependencies,
            readme: krate.readme,
            rustdoc: krate.description_long,
            release_time: krate.release_time,
            build_status: krate.build_status,
            latest_build_id: krate.latest_build_id,
            last_successful_build: None,
            rustdoc_status: krate.rustdoc_status,
            archive_storage: krate.archive_storage,
            repository_url: krate.repository_url,
            homepage_url: krate.homepage_url,
            keywords: krate.keywords,
            have_examples: krate.have_examples,
            target_name: krate.target_name,
            releases,
            repository_metadata,
            metadata,
            is_library: krate.is_library,
            license: krate.license,
            documentation_url: krate.documentation_url,
            documented_items: krate.documented_items,
            total_items: krate.total_items,
            total_items_needing_examples: krate.total_items_needing_examples,
            items_with_examples: krate.items_with_examples,
            crate_id: krate.crate_id,
            release_id: krate.release_id,
        };

        // get owners
        crate_details.owners = sqlx::query!(
            "SELECT login, avatar
             FROM owners
             INNER JOIN owner_rels ON owner_rels.oid = owners.id
             WHERE cid = $1",
            krate.crate_id,
        )
        .fetch(&mut *conn)
        .map_ok(|row| (row.login, row.avatar))
        .try_collect()
        .await?;

        if !crate_details.build_status {
            crate_details.last_successful_build = crate_details
                .releases
                .iter()
                .filter(|release| release.build_status && !release.yanked)
                .map(|release| release.version.to_string())
                .next();
        }

        Ok(Some(crate_details))
    }

    #[fn_error_context::context("fetching readme for {} {}", self.name, self.version)]
    async fn fetch_readme(&self, storage: &AsyncStorage) -> anyhow::Result<Option<String>> {
        let manifest = match storage
            .fetch_source_file(
                &self.name,
                &self.version,
                self.latest_build_id.unwrap_or(0),
                "Cargo.toml",
                self.archive_storage,
            )
            .await
        {
            Ok(manifest) => manifest,
            Err(err) if err.is::<PathNotFoundError>() => {
                return Ok(None);
            }
            Err(err) => {
                return Err(err);
            }
        };
        let manifest = String::from_utf8(manifest.content)
            .context("parsing Cargo.toml")?
            .parse::<toml::Value>()
            .context("parsing Cargo.toml")?;
        let paths = match manifest.get("package").and_then(|p| p.get("readme")) {
            Some(toml::Value::Boolean(true)) => vec!["README.md"],
            Some(toml::Value::Boolean(false)) => vec![],
            Some(toml::Value::String(path)) => vec![path.as_ref()],
            _ => vec!["README.md", "README.txt", "README"],
        };
        for path in &paths {
            match storage
                .fetch_source_file(
                    &self.name,
                    &self.version,
                    self.latest_build_id.unwrap_or(0),
                    path,
                    self.archive_storage,
                )
                .await
            {
                Ok(readme) => {
                    let readme = String::from_utf8(readme.content)
                        .with_context(|| format!("parsing {path} content"))?;
                    return Ok(Some(readme));
                }
                Err(err) if err.is::<PathNotFoundError>() => {
                    continue;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
        Ok(None)
    }

    /// Returns the latest non-yanked, non-prerelease release of this crate (or latest
    /// yanked/prereleased if that is all that exist).
    pub fn latest_release(&self) -> Result<&Release> {
        latest_release(&self.releases).ok_or_else(|| anyhow!("crate without releases"))
    }
}

pub(crate) fn latest_release(releases: &[Release]) -> Option<&Release> {
    if let Some(release) = releases
        .iter()
        .find(|release| release.version.pre.is_empty() && !release.yanked)
    {
        Some(release)
    } else {
        releases.first()
    }
}

/// Return all releases for a crate, sorted in descending order by semver
pub(crate) async fn releases_for_crate(
    conn: &mut sqlx::PgConnection,
    crate_id: i32,
) -> Result<Vec<Release>, anyhow::Error> {
    let mut releases: Vec<Release> = sqlx::query!(
        "SELECT
            id,
            version,
            build_status,
            yanked,
            is_library,
            rustdoc_status,
            target_name
         FROM releases
         WHERE
             releases.crate_id = $1",
        crate_id,
    )
    .fetch(&mut *conn)
    .try_filter_map(|row| async move {
        Ok(
            match semver::Version::parse(&row.version).with_context(|| {
                format!(
                    "invalid semver in database for crate {crate_id}: {}",
                    row.version
                )
            }) {
                Ok(semversion) => Some(Release {
                    id: row.id,
                    version: semversion,
                    build_status: row.build_status,
                    yanked: row.yanked,
                    is_library: row.is_library,
                    rustdoc_status: row.rustdoc_status,
                    target_name: row.target_name,
                }),
                Err(err) => {
                    report_error(&err);
                    None
                }
            },
        )
    })
    .try_collect()
    .await?;

    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct CrateDetailsPage {
    details: CrateDetails,
}

impl_axum_webpage! {
    CrateDetailsPage = "crate/details.html",
    cpu_intensive_rendering = true,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct CrateDetailHandlerParams {
    name: String,
    version: Option<String>,
}

#[tracing::instrument(skip(conn, storage))]
pub(crate) async fn crate_details_handler(
    Path(params): Path<CrateDetailHandlerParams>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    // this handler must always called with a crate name
    if params.version.is_none() {
        return Ok(super::axum_cached_redirect(
            encode_url_path(&format!("/crate/{}/latest", params.name)),
            CachePolicy::ForeverInCdn,
        )?
        .into_response());
    }

    let found_version = match_version(&mut conn, &params.name, params.version.as_deref())
        .await
        .and_then(|m| m.exact_name_only())?;

    let (version, version_or_latest, is_latest_url) = match found_version {
        MatchSemver::Exact((version, _)) => (version.clone(), version, false),
        MatchSemver::Latest((version, _)) => (version, "latest".to_string(), true),
        MatchSemver::Semver((version, _)) => {
            return Ok(super::axum_cached_redirect(
                &format!("/crate/{}/{}", &params.name, version),
                CachePolicy::ForeverInCdn,
            )?
            .into_response());
        }
    };

    let mut details = CrateDetails::new(&mut conn, &params.name, &version, &version_or_latest)
        .await?
        .ok_or(AxumNope::VersionNotFound)?;

    match details.fetch_readme(&storage).await {
        Ok(readme) => details.readme = readme.or(details.readme),
        Err(e) => warn!("error fetching readme: {:?}", &e),
    }

    let mut res = CrateDetailsPage { details }.into_response();
    res.extensions_mut()
        .insert::<CachePolicy>(if is_latest_url {
            CachePolicy::ForeverInCdn
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser
        });
    Ok(res.into_response())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ReleaseList {
    releases: Vec<Release>,
    crate_name: String,
    inner_path: String,
    target: String,
}

impl_axum_webpage! {
    ReleaseList = "rustdoc/releases.html",
    cache_policy = |_| CachePolicy::ForeverInCdn,
    cpu_intensive_rendering = true,
}

#[tracing::instrument]
pub(crate) async fn get_all_releases(
    Path(params): Path<RustdocHtmlParams>,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    let req_path: String = params.path.clone().unwrap_or_default();
    let req_path: Vec<&str> = req_path.split('/').collect();

    let release_found = match_version(&mut conn, &params.name, Some(&params.version)).await?;
    trace!(?release_found, "found release");

    let (version, _) = match release_found.version {
        MatchSemver::Exact((version, _)) => (version.clone(), version),
        MatchSemver::Latest((version, _)) => (version, "latest".to_string()),
        MatchSemver::Semver(_) => return Err(AxumNope::VersionNotFound),
    };

    let row = sqlx::query!(
        "SELECT
            crates.id AS crate_id,
            releases.doc_targets,
            releases.target_name
        FROM crates
        INNER JOIN releases on crates.id = releases.crate_id
        WHERE crates.name = $1 and releases.version = $2;",
        params.name,
        &version,
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::CrateNotFound)?;

    // get releases, sorted by semver
    let releases: Vec<Release> = releases_for_crate(&mut conn, row.crate_id).await?;

    let doc_targets = MetaData::parse_doc_targets(row.doc_targets);

    let inner;
    let (target, inner_path) = {
        let mut inner_path = req_path.clone();

        let target = if inner_path.len() > 1
            && doc_targets
                .iter()
                .any(|s| Some(s) == params.target.as_ref())
        {
            inner_path.remove(0);
            params.target.as_ref().unwrap()
        } else {
            ""
        };

        inner = inner_path.join("/");
        (target, inner.trim_end_matches('/'))
    };
    let inner_path = if inner_path.is_empty() {
        format!("{}/index.html", row.target_name)
    } else {
        format!("{}/{inner_path}", row.target_name)
    };

    let target = if target.is_empty() {
        String::new()
    } else {
        format!("{target}/")
    };

    let res = ReleaseList {
        releases,
        target,
        inner_path,
        crate_name: params.name,
    };
    Ok(res.into_response())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ShortMetadata {
    name: String,
    version_or_latest: String,
    doc_targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct PlatformList {
    metadata: ShortMetadata,
    inner_path: String,
    use_direct_platform_links: bool,
    current_target: String,
}

impl_axum_webpage! {
    PlatformList = "rustdoc/platforms.html",
    cache_policy = |_| CachePolicy::ForeverInCdn,
    cpu_intensive_rendering = true,
}

#[tracing::instrument]
pub(crate) async fn get_all_platforms_inner(
    Path(params): Path<RustdocHtmlParams>,
    mut conn: DbConnection,
    is_crate_root: bool,
) -> AxumResult<AxumResponse> {
    let req_path: String = params.path.unwrap_or_default();
    let req_path: Vec<&str> = req_path.split('/').collect();

    let release_found = match_version(&mut conn, &params.name, Some(&params.version)).await?;
    trace!(?release_found, "found release");

    // Convenience function to allow for easy redirection
    #[instrument]
    fn redirect(
        name: &str,
        vers: &str,
        path: &[&str],
        cache_policy: CachePolicy,
    ) -> AxumResult<AxumResponse> {
        trace!("redirect");
        // Format and parse the redirect url
        Ok(axum_cached_redirect(
            encode_url_path(&format!("/platforms/{}/{}/{}", name, vers, path.join("/"))),
            cache_policy,
        )?
        .into_response())
    }

    let (version, version_or_latest) = match release_found.version {
        MatchSemver::Exact((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, &version, &req_path, CachePolicy::NoCaching);
            }

            (version.clone(), version)
        }

        MatchSemver::Latest((version, _)) => {
            // Redirect when the requested crate name isn't correct
            if let Some(name) = release_found.corrected_name {
                return redirect(&name, "latest", &req_path, CachePolicy::NoCaching);
            }

            (version, "latest".to_string())
        }

        // Redirect when the requested version isn't correct
        MatchSemver::Semver((v, _)) => {
            // to prevent cloudfront caching the wrong artifacts on URLs with loose semver
            // versions, redirect the browser to the returned version instead of loading it
            // immediately
            return redirect(&params.name, &v, &req_path, CachePolicy::ForeverInCdn);
        }
    };

    let krate = sqlx::query!(
        "SELECT
            crates.id,
            crates.name,
            releases.default_target,
            releases.doc_targets
        FROM releases
        INNER JOIN crates ON releases.crate_id = crates.id
        WHERE crates.name = $1 AND releases.version = $2;",
        params.name,
        version
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::CrateNotFound)?;

    let releases = releases_for_crate(&mut conn, krate.id).await?;

    let doc_targets = MetaData::parse_doc_targets(krate.doc_targets);

    let latest_release = releases
        .iter()
        .find(|release| release.version.pre.is_empty() && !release.yanked)
        .unwrap_or(&releases[0]);

    // The path within this crate version's rustdoc output
    let inner;
    let (target, inner_path) = {
        let mut inner_path = req_path.clone();

        let target = if inner_path.len() > 1
            && doc_targets
                .iter()
                .any(|s| Some(s) == params.target.as_ref())
        {
            inner_path.remove(0);
            params.target.as_ref().unwrap()
        } else {
            ""
        };

        inner = inner_path.join("/");
        (target, inner.trim_end_matches('/'))
    };
    let inner_path = if inner_path.is_empty() {
        format!("{}/index.html", krate.name)
    } else {
        format!("{}/{inner_path}", krate.name)
    };

    let current_target = if latest_release.build_status {
        if target.is_empty() {
            krate.default_target
        } else {
            target.to_owned()
        }
    } else {
        String::new()
    };

    let res = PlatformList {
        metadata: ShortMetadata {
            name: krate.name,
            version_or_latest: version_or_latest.to_string(),
            doc_targets,
        },
        inner_path,
        use_direct_platform_links: is_crate_root,
        current_target,
    };
    Ok(res.into_response())
}

pub(crate) async fn get_all_platforms_root(
    Path(mut params): Path<RustdocHtmlParams>,
    conn: DbConnection,
) -> AxumResult<AxumResponse> {
    params.path = None;
    get_all_platforms_inner(Path(params), conn, true).await
}

pub(crate) async fn get_all_platforms(
    params: Path<RustdocHtmlParams>,
    conn: DbConnection,
) -> AxumResult<AxumResponse> {
    get_all_platforms_inner(params, conn, false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry_api::CrateOwner;
    use crate::test::{
        assert_cache_control, assert_redirect, assert_redirect_cached, async_wrapper, wrapper,
        TestDatabase, TestEnvironment,
    };
    use anyhow::{Context, Error};
    use kuchikiki::traits::TendrilSink;
    use std::collections::HashMap;

    async fn assert_last_successful_build_equals(
        db: &TestDatabase,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<&str>,
    ) -> Result<(), Error> {
        let mut conn = db.async_conn().await;
        let details = CrateDetails::new(&mut conn, package, version, version)
            .await
            .with_context(|| anyhow::anyhow!("could not fetch crate details"))?
            .unwrap();

        assert_eq!(
            details.last_successful_build,
            expected_last_successful_build.map(|s| s.to_string()),
        );
        Ok(())
    }

    #[test]
    fn test_last_successful_build_when_last_releases_failed_or_yanked() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .build_result_failed()
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.4")
                .yanked(true)
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.5")
                .build_result_failed()
                .yanked(true)
                .create_async()
                .await?;

            assert_last_successful_build_equals(db, "foo", "0.0.1", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.2", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.3", Some("0.0.2")).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.4", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.5", Some("0.0.2")).await?;
            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_when_all_releases_failed_or_yanked() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .build_result_failed()
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .build_result_failed()
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create_async()
                .await?;

            assert_last_successful_build_equals(db, "foo", "0.0.1", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.2", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.3", None).await?;
            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_with_intermittent_releases_failed_or_yanked() {
        async_wrapper(|env| async move {
            let db = env.async_db().await;

            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .build_result_failed()
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create_async()
                .await?;
            env.async_fake_release()
                .await
                .name("foo")
                .version("0.0.4")
                .create_async()
                .await?;

            assert_last_successful_build_equals(db, "foo", "0.0.1", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.2", Some("0.0.4")).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.3", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.4", None).await?;
            Ok(())
        });
    }

    #[test]
    fn test_releases_should_be_sorted() {
        wrapper(|env| {
            let db = env.db();

            // Add new releases of 'foo' out-of-order since CrateDetails should sort them descending
            env.fake_release().name("foo").version("0.1.0").create()?;
            env.fake_release().name("foo").version("0.1.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.3.0")
                .build_result_failed()
                .create()?;
            env.fake_release().name("foo").version("1.0.0").create()?;
            env.fake_release().name("foo").version("0.12.0").create()?;
            env.fake_release()
                .name("foo")
                .version("0.2.0")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.2.0-alpha")
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .build_result_failed()
                .binary(true)
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                CrateDetails::new(&mut conn, "foo", "0.2.0", "0.2.0")
                    .await
                    .unwrap()
                    .unwrap()
            });

            assert_eq!(
                details.releases,
                vec![
                    Release {
                        version: semver::Version::parse("1.0.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[0].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.12.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[1].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.3.0")?,
                        build_status: false,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: false,
                        id: details.releases[2].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.2.0")?,
                        build_status: true,
                        yanked: true,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[3].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.2.0-alpha")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[4].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.1.1")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[5].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.1.0")?,
                        build_status: true,
                        yanked: false,
                        is_library: true,
                        rustdoc_status: true,
                        id: details.releases[6].id,
                        target_name: "foo".to_owned(),
                    },
                    Release {
                        version: semver::Version::parse("0.0.1")?,
                        build_status: false,
                        yanked: false,
                        is_library: false,
                        rustdoc_status: false,
                        id: details.releases[7].id,
                        target_name: "foo".to_owned(),
                    },
                ]
            );

            Ok(())
        });
    }

    #[test]
    fn test_canonical_url() {
        wrapper(|env| {
            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            let response = env.frontend().get("/crate/foo/0.0.1").send()?;
            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );

            assert!(response
                .text()?
                .contains("rel=\"canonical\" href=\"https://docs.rs/crate/foo/latest"));

            Ok(())
        })
    }

    #[test]
    fn test_latest_version() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release().name("foo").version("0.0.3").create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = env.runtime().block_on(async move {
                    let mut conn = db.async_conn().await;
                    CrateDetails::new(&mut conn, "foo", version, version)
                        .await
                        .unwrap()
                        .unwrap()
                });
                assert_eq!(
                    details.latest_release().unwrap().version,
                    semver::Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_prerelease() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3-pre.1")
                .create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3-pre.1"] {
                let details = env.runtime().block_on(async move {
                    let mut conn = db.async_conn().await;
                    CrateDetails::new(&mut conn, "foo", version, version)
                        .await
                        .unwrap()
                        .unwrap()
                });
                assert_eq!(
                    details.latest_release().unwrap().version,
                    semver::Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_yanked() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            env.fake_release().name("foo").version("0.0.2").create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = env.runtime().block_on(async move {
                    let mut conn = db.async_conn().await;
                    CrateDetails::new(&mut conn, "foo", version, version)
                        .await
                        .unwrap()
                        .unwrap()
                });
                assert_eq!(
                    details.latest_release().unwrap().version,
                    semver::Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_only_yanked() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .yanked(true)
                .create()?;

            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = env.runtime().block_on(async move {
                    let mut conn = db.async_conn().await;
                    CrateDetails::new(&mut conn, "foo", version, version)
                        .await
                        .unwrap()
                        .unwrap()
                });
                assert_eq!(
                    details.latest_release().unwrap().version,
                    semver::Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn releases_dropdowns_is_correct() {
        wrapper(|env| {
            env.fake_release()
                .name("binary")
                .version("0.1.0")
                .binary(true)
                .create()?;

            let page = kuchikiki::parse_html()
                .one(env.frontend().get("/crate/binary/0.1.0").send()?.text()?);
            let warning = page.select_first("a.pure-menu-link.warn").unwrap();

            assert_eq!(
                warning
                    .as_node()
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow()
                    .get("title")
                    .unwrap(),
                "binary-0.1.0 is not a library"
            );

            Ok(())
        });
    }

    #[test]
    fn test_updating_owners() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release()
                .name("foo")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobar".into(),
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                CrateDetails::new(&mut conn, "foo", "0.0.1", "0.0.1")
                    .await
                    .unwrap()
                    .unwrap()
            });
            assert_eq!(
                details.owners,
                vec![("foobar".into(), "https://example.org/foobar".into())]
            );

            // Adding a new owner, and changing details on an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobarv2".into(),
                })
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                CrateDetails::new(&mut conn, "foo", "0.0.1", "0.0.1")
                    .await
                    .unwrap()
                    .unwrap()
            });
            let mut owners = details.owners;
            owners.sort();
            assert_eq!(
                owners,
                vec![
                    ("barfoo".into(), "https://example.org/barfoo".into()),
                    ("foobar".into(), "https://example.org/foobarv2".into())
                ]
            );

            // Removing an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                CrateDetails::new(&mut conn, "foo", "0.0.1", "0.0.1")
                    .await
                    .unwrap()
                    .unwrap()
            });
            assert_eq!(
                details.owners,
                vec![("barfoo".into(), "https://example.org/barfoo".into())]
            );

            // Changing owner details on another of their crates applies the change to both
            env.fake_release()
                .name("bar")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoov2".into(),
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                CrateDetails::new(&mut conn, "foo", "0.0.1", "0.0.1")
                    .await
                    .unwrap()
                    .unwrap()
            });
            assert_eq!(
                details.owners,
                vec![("barfoo".into(), "https://example.org/barfoov2".into())]
            );

            Ok(())
        });
    }

    #[test]
    fn feature_flags_report_empty() {
        wrapper(|env| {
            env.fake_release()
                .name("library")
                .version("0.1.0")
                .features(HashMap::new())
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/library/0.1.0/features")
                    .send()?
                    .text()?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn feature_private_feature_flags_are_hidden() {
        wrapper(|env| {
            let features = [("_private".into(), Vec::new())]
                .iter()
                .cloned()
                .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/library/0.1.0/features")
                    .send()?
                    .text()?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn feature_flags_without_default() {
        wrapper(|env| {
            let features = [("feature1".into(), Vec::new())]
                .iter()
                .cloned()
                .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/library/0.1.0/features")
                    .send()?
                    .text()?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_err());
            let def_len = page
                .select_first(r#"b[data-id="default-feature-len"]"#)
                .unwrap();
            assert_eq!(def_len.text_contents(), "0");
            Ok(())
        });
    }

    #[test]
    fn feature_flags_with_nested_default() {
        wrapper(|env| {
            let features = [
                ("default".into(), vec!["feature1".into()]),
                ("feature1".into(), vec!["feature2".into()]),
                ("feature2".into(), Vec::new()),
            ]
            .iter()
            .cloned()
            .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/library/0.1.0/features")
                    .send()?
                    .text()?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_err());
            let def_len = page
                .select_first(r#"b[data-id="default-feature-len"]"#)
                .unwrap();
            assert_eq!(def_len.text_contents(), "3");
            Ok(())
        });
    }

    #[test]
    fn feature_flags_report_null() {
        wrapper(|env| {
            let id = env
                .fake_release()
                .name("library")
                .version("0.1.0")
                .create()?;

            env.db()
                .conn()
                .query("UPDATE releases SET features = NULL WHERE id = $1", &[&id])?;

            let page = kuchikiki::parse_html().one(
                env.frontend()
                    .get("/crate/library/0.1.0/features")
                    .send()?
                    .text()?,
            );
            assert!(page.select_first(r#"p[data-id="null-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn platform_links_are_direct_and_without_nofollow() {
        fn check_links(
            response_text: String,
            ajax: bool,
            should_contain_redirect: bool,
        ) -> Vec<(String, String, String)> {
            let platform_links: Vec<(String, String, String)> = kuchikiki::parse_html()
                .one(response_text)
                .select(&format!(r#"{}li a"#, if ajax { "" } else { "#platforms " }))
                .expect("invalid selector")
                .map(|el| {
                    let attributes = el.attributes.borrow();
                    let url = attributes.get("href").expect("href").to_string();
                    let rel = attributes.get("rel").unwrap_or("").to_string();
                    (el.text_contents(), url, rel)
                })
                .collect();

            assert_eq!(platform_links.len(), 2);

            for (_, url, rel) in &platform_links {
                assert_eq!(
                    url.contains("/target-redirect/"),
                    should_contain_redirect,
                    "ajax: {ajax:?}, should_contain_redirect: {should_contain_redirect:?}",
                );
                if !should_contain_redirect {
                    assert_eq!(rel, "");
                } else {
                    assert_eq!(rel, "nofollow");
                }
            }
            platform_links
        }

        fn run_check_links_redir(
            env: &TestEnvironment,
            url_start: &str,
            url_end: &str,
            extra: &str,
            should_contain_redirect: bool,
        ) {
            let response = env
                .frontend()
                .get(&format!("{url_start}{url_end}"))
                .send()
                .unwrap();
            assert!(response.status().is_success());
            let list1 = check_links(response.text().unwrap(), false, should_contain_redirect);
            // Same test with AJAX endpoint.
            let (start, extra_name) = if url_start.starts_with("/crate/") {
                ("", "/crate")
            } else {
                ("/crate", "")
            };
            let response = env
                .frontend()
                .get(&format!(
                    "{start}{url_start}/menus/platforms{extra_name}{url_end}{extra}"
                ))
                .send()
                .unwrap();
            assert!(response.status().is_success());
            assert_cache_control(&response, CachePolicy::ForeverInCdn, &env.config());
            let list2 = check_links(response.text().unwrap(), true, should_contain_redirect);
            assert_eq!(list1, list2);
        }

        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/struct.A.html")
                .default_target("x86_64-unknown-linux-gnu")
                .add_target("x86_64-pc-windows-msvc")
                .source_file("README.md", b"storage readme")
                .create()?;

            // FIXME: For some reason, there are target-redirects on non-AJAX lists on docs.rs
            // crate pages other than the "default" one.
            run_check_links_redir(env, "/crate/dummy/0.4.0", "/features", "", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0", "/builds", "", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0", "/source/", "", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0", "/source/README.md", "", false);

            run_check_links_redir(env, "/crate/dummy/0.4.0", "", "/", false);
            run_check_links_redir(env, "/dummy/latest", "/dummy", "/", true);
            run_check_links_redir(
                env,
                "/dummy/0.4.0",
                "/x86_64-pc-windows-msvc/dummy",
                "/",
                true,
            );
            run_check_links_redir(
                env,
                "/dummy/0.4.0",
                "/x86_64-pc-windows-msvc/dummy/struct.A.html",
                "/",
                true,
            );

            Ok(())
        });
    }

    #[test]
    fn check_crate_name_in_redirect() {
        fn check_links(env: &TestEnvironment, url: &str, links: Vec<String>) {
            let response = env.frontend().get(url).send().unwrap();
            assert!(response.status().is_success());

            let platform_links: Vec<String> = kuchikiki::parse_html()
                .one(response.text().unwrap())
                .select("li a")
                .expect("invalid selector")
                .map(|el| {
                    let attributes = el.attributes.borrow();
                    let url = attributes.get("href").expect("href").to_string();
                    url
                })
                .collect();

            assert_eq!(platform_links, links,);
        }

        wrapper(|env| {
            env.fake_release()
                .name("dummy-ba")
                .version("0.4.0")
                .rustdoc_file("dummy-ba/index.html")
                .rustdoc_file("x86_64-unknown-linux-gnu/dummy-ba/index.html")
                .add_target("x86_64-unknown-linux-gnu")
                .create()?;
            env.fake_release()
                .name("dummy-ba")
                .version("0.5.0")
                .rustdoc_file("dummy-ba/index.html")
                .rustdoc_file("x86_64-unknown-linux-gnu/dummy-ba/index.html")
                .add_target("x86_64-unknown-linux-gnu")
                .create()?;

            check_links(
                env,
                "/crate/dummy-ba/latest/menus/releases/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/dummy_ba/index.html".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/dummy_ba/index.html".to_string(),
                ],
            );

            check_links(
                env,
                "/crate/dummy-ba/latest/menus/releases/x86_64-unknown-linux-gnu/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/index.html".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/index.html".to_string(),
                ],
            );

            Ok(())
        });
    }

    // Ensure that if there are more than a given number of targets, it will not generate them in
    // the HTML directly (they will be loaded by AJAX if the user opens the menu).
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn platform_menu_ajax() {
        assert!(crate::DEFAULT_MAX_TARGETS > 2);

        fn check_count(nb_targets: usize, expected: usize) {
            wrapper(|env| {
                let mut rel = env
                    .fake_release()
                    .name("dummy")
                    .version("0.4.0")
                    .rustdoc_file("dummy/index.html")
                    .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                    .default_target("x86_64-unknown-linux-gnu");

                for nb in 0..nb_targets - 1 {
                    rel = rel.add_target(&format!("x86_64-pc-windows-msvc{nb}"));
                }
                rel.create()?;

                let response = env.frontend().get("/crate/dummy/0.4.0").send()?;
                assert!(response.status().is_success());

                let nb_li = kuchikiki::parse_html()
                    .one(response.text()?)
                    .select(r#"#platforms li a"#)
                    .expect("invalid selector")
                    .count();
                assert_eq!(nb_li, expected);
                Ok(())
            });
        }

        // First we check that with 2 releases, the platforms list should be in the HTML.
        check_count(2, 2);
        // Then we check the same thing but with number of targets equal
        // to `DEFAULT_MAX_TARGETS`.
        check_count(crate::DEFAULT_MAX_TARGETS, 0);
    }

    #[test]
    fn latest_url() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                .default_target("x86_64-unknown-linux-gnu")
                .add_target("x86_64-pc-windows-msvc")
                .create()?;
            let web = env.frontend();

            let resp = env.frontend().get("/crate/dummy/latest").send()?;
            assert!(resp.status().is_success());
            assert_cache_control(&resp, CachePolicy::ForeverInCdn, &env.config());
            assert!(resp.url().as_str().ends_with("/crate/dummy/latest"));
            let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
            assert!(body.contains("<a href=\"/crate/dummy/latest/features\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest\""));

            assert_redirect("/crate/dummy/latest/", "/crate/dummy/latest", web)?;
            assert_redirect_cached(
                "/crate/dummy",
                "/crate/dummy/latest",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;

            let resp_json = env
                .frontend()
                .get("/crate/aquarelle/latest/builds.json")
                .send()?;
            assert!(resp_json
                .url()
                .as_str()
                .ends_with("/crate/aquarelle/latest/builds.json"));

            Ok(())
        });
    }

    #[test]
    fn readme() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .readme_only_database("database readme")
                .create()?;

            env.fake_release()
                .name("dummy")
                .version("0.2.0")
                .readme_only_database("database readme")
                .source_file("README.md", b"storage readme")
                .create()?;

            env.fake_release()
                .name("dummy")
                .version("0.3.0")
                .source_file("README.md", b"storage readme")
                .create()?;

            env.fake_release()
                .name("dummy")
                .version("0.4.0")
                .readme_only_database("database readme")
                .source_file("MEREAD", b"storage meread")
                .source_file("Cargo.toml", br#"package.readme = "MEREAD""#)
                .create()?;

            env.fake_release()
                .name("dummy")
                .version("0.5.0")
                .readme_only_database("database readme")
                .source_file("README.md", b"storage readme")
                .no_cargo_toml()
                .create()?;

            let check_readme = |path, content| {
                let resp = env.frontend().get(path).send().unwrap();
                let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
                assert!(body.contains(content));
            };

            check_readme("/crate/dummy/0.1.0", "database readme");
            check_readme("/crate/dummy/0.2.0", "storage readme");
            check_readme("/crate/dummy/0.3.0", "storage readme");
            check_readme("/crate/dummy/0.4.0", "storage meread");

            let details = env.runtime().block_on(async move {
                let mut conn = env.async_db().await.async_conn().await;
                CrateDetails::new(&mut conn, "dummy", "0.5.0", "0.5.0")
                    .await
                    .unwrap()
                    .unwrap()
            });
            assert!(matches!(
                env.runtime()
                    .block_on(details.fetch_readme(&env.runtime().block_on(env.async_storage()))),
                Ok(None)
            ));
            Ok(())
        });
    }
}
