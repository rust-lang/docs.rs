use super::{markdown, match_version, MetaData};
use crate::registry_api::OwnerKind;
use crate::utils::{get_correct_docsrs_style_file, report_error};
use crate::web::rustdoc::RustdocHtmlParams;
use crate::{
    db::types::BuildStatus,
    impl_axum_webpage,
    storage::PathNotFoundError,
    web::{
        cache::CachePolicy,
        encode_url_path,
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        page::templates::filters,
        MatchedRelease, ReqVersion,
    },
    AsyncStorage,
};
use anyhow::{anyhow, Context, Result};
use axum::{
    extract::Extension,
    response::{IntoResponse, Response as AxumResponse},
};
use chrono::{DateTime, Utc};
use futures_util::stream::TryStreamExt;
use log::warn;
use rinja::Template;
use semver::Version;
use serde::Deserialize;
use serde::{ser::Serializer, Serialize};
use serde_json::Value;
use std::sync::Arc;

// TODO: Add target name and versions

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CrateDetails {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) description: Option<String>,
    pub(crate) owners: Vec<(String, String, OwnerKind)>,
    pub(crate) dependencies: Option<Value>,
    #[serde(serialize_with = "optional_markdown")]
    readme: Option<String>,
    #[serde(serialize_with = "optional_markdown")]
    rustdoc: Option<String>, // this is description_long in database
    release_time: Option<DateTime<Utc>>,
    build_status: BuildStatus,
    pub latest_build_id: Option<i32>,
    last_successful_build: Option<String>,
    pub rustdoc_status: Option<bool>,
    pub archive_storage: bool,
    pub repository_url: Option<String>,
    pub homepage_url: Option<String>,
    keywords: Option<Value>,
    have_examples: Option<bool>, // need to check this manually
    pub target_name: Option<String>,
    releases: Vec<Release>,
    repository_metadata: Option<RepositoryMetadata>,
    pub(crate) metadata: MetaData,
    is_library: Option<bool>,
    pub(crate) license: Option<String>,
    pub(crate) documentation_url: Option<String>,
    pub(crate) total_items: Option<i32>,
    pub(crate) documented_items: Option<i32>,
    pub(crate) total_items_needing_examples: Option<i32>,
    pub(crate) items_with_examples: Option<i32>,
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
pub(crate) struct Release {
    pub id: i32,
    pub version: semver::Version,
    /// Aggregated build status of the release.
    /// * no builds -> build In progress
    /// * any build is successful -> Success
    ///   -> even with failed or in-progress builds we have docs to show
    /// * any build is failed -> Failure
    ///   -> we can only have Failure or InProgress here, so the Failure is the
    ///      important part on this aggregation level.
    /// * the rest is all builds are in-progress -> InProgress
    ///   -> if we have any builds, and the previous conditions don't match, we end
    ///      up here, but we still check.
    /// calculated in a database view : `release_build_status`
    pub build_status: BuildStatus,
    pub yanked: Option<bool>,
    pub is_library: Option<bool>,
    pub rustdoc_status: Option<bool>,
    pub target_name: Option<String>,
}

impl CrateDetails {
    #[tracing::instrument(skip(conn))]
    pub(crate) async fn from_matched_release(
        conn: &mut sqlx::PgConnection,
        release: MatchedRelease,
    ) -> Result<Self> {
        Ok(Self::new(
            conn,
            &release.corrected_name.unwrap_or(release.name),
            &release.release.version,
            Some(release.req_version),
            release.all_releases,
        )
        .await?
        .unwrap())
    }

    async fn new(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &Version,
        req_version: Option<ReqVersion>,
        prefetched_releases: Vec<Release>,
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
                release_build_status.build_status as "build_status!: BuildStatus",
                (
                    -- this is the latest build ID that generated content
                    -- it's used to invalidate some blob storage related caches.
                    SELECT id
                    FROM builds
                    WHERE
                        builds.rid = releases.id AND
                        builds.build_status = 'success'
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
                (
                    -- we're using the rustc version here to set the correct CSS file
                    -- in the metadata.
                    -- So we're only interested in successful builds here.
                    SELECT rustc_version
                    FROM builds
                    WHERE
                        builds.rid = releases.id AND
                        builds.build_status = 'success'
                    ORDER BY builds.build_time
                    DESC LIMIT 1
                ) as "rustc_version?",
                doc_coverage.total_items,
                doc_coverage.documented_items,
                doc_coverage.total_items_needing_examples,
                doc_coverage.items_with_examples
            FROM releases
            INNER JOIN release_build_status ON releases.id = release_build_status.rid
            INNER JOIN crates ON releases.crate_id = crates.id
            LEFT JOIN doc_coverage ON doc_coverage.release_id = releases.id
            LEFT JOIN repositories ON releases.repository_id = repositories.id
            WHERE crates.name = $1 AND releases.version = $2;"#,
            name,
            version.to_string(),
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            Some(row) => row,
            None => return Ok(None),
        };

        let repository_metadata = krate.repo_host.map(|_| RepositoryMetadata {
            issues: krate.repo_issues.unwrap(),
            stars: krate.repo_stars.unwrap(),
            forks: krate.repo_forks.unwrap(),
            name: krate.repo_name,
        });

        let metadata = MetaData {
            name: krate.name.clone(),
            version: version.clone(),
            req_version: req_version.unwrap_or_else(|| ReqVersion::Exact(version.clone())),
            description: krate.description.clone(),
            rustdoc_status: krate.rustdoc_status,
            target_name: krate.target_name.clone(),
            default_target: krate.default_target,
            doc_targets: krate.doc_targets.map(MetaData::parse_doc_targets),
            yanked: krate.yanked,
            rustdoc_css_file: krate
                .rustc_version
                .as_deref()
                .map(get_correct_docsrs_style_file)
                .transpose()?,
        };

        let mut crate_details = CrateDetails {
            name: krate.name,
            version: version.clone(),
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
            releases: prefetched_releases,
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
            r#"SELECT login, avatar, kind as "kind: OwnerKind"
             FROM owners
             INNER JOIN owner_rels ON owner_rels.oid = owners.id
             WHERE cid = $1"#,
            krate.crate_id,
        )
        .fetch(&mut *conn)
        .map_ok(|row| (row.login, row.avatar, row.kind))
        .try_collect()
        .await?;

        if crate_details.build_status != BuildStatus::Success {
            crate_details.last_successful_build = crate_details
                .releases
                .iter()
                .filter(|release| {
                    release.build_status == BuildStatus::Success && release.yanked == Some(false)
                })
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
                &self.version.to_string(),
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
                    &self.version.to_string(),
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
    if let Some(release) = releases.iter().find(|release| {
        release.version.pre.is_empty()
            && release.yanked == Some(false)
            && release.build_status != BuildStatus::InProgress
    }) {
        Some(release)
    } else {
        releases
            .iter()
            .find(|release| release.build_status != BuildStatus::InProgress)
    }
}

/// Return all releases for a crate, sorted in descending order by semver
pub(crate) async fn releases_for_crate(
    conn: &mut sqlx::PgConnection,
    crate_id: i32,
) -> Result<Vec<Release>, anyhow::Error> {
    let mut releases: Vec<Release> = sqlx::query!(
        r#"SELECT
             releases.id,
             releases.version,
             release_build_status.build_status as "build_status!: BuildStatus",
             releases.yanked,
             releases.is_library,
             releases.rustdoc_status,
             releases.target_name
         FROM releases
         INNER JOIN release_build_status ON releases.id = release_build_status.rid
         WHERE
             releases.crate_id = $1 AND
             release_build_status.build_status != 'in_progress'"#,
        crate_id,
    )
    .fetch(&mut *conn)
    .try_filter_map(|row| async move {
        let semversion = match semver::Version::parse(&row.version).with_context(|| {
            format!(
                "invalid semver in database for crate {crate_id}: {}",
                row.version
            )
        }) {
            Ok(semver) => semver,
            Err(err) => {
                report_error(&err);
                return Ok(None);
            }
        };

        Ok(Some(Release {
            id: row.id,
            version: semversion,
            build_status: row.build_status,
            yanked: row.yanked,
            is_library: row.is_library,
            rustdoc_status: row.rustdoc_status,
            target_name: row.target_name,
        }))
    })
    .try_collect()
    .await?;

    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
}

#[derive(Template)]
#[template(path = "crate/details.html")]
#[derive(Debug, Clone, PartialEq, Serialize)]
struct CrateDetailsPage {
    details: CrateDetails,
    csp_nonce: String,
}

impl_axum_webpage! {
    CrateDetailsPage,
    cpu_intensive_rendering = true,
}

// Used by templates.
impl CrateDetailsPage {
    pub(crate) fn krate(&self) -> Option<&CrateDetails> {
        None
    }

    pub(crate) fn permalink_path(&self) -> &str {
        ""
    }

    pub(crate) fn get_metadata(&self) -> Option<&MetaData> {
        Some(&self.details.metadata)
    }

    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct CrateDetailHandlerParams {
    name: String,
    version: Option<ReqVersion>,
}

#[tracing::instrument(skip(conn, storage))]
pub(crate) async fn crate_details_handler(
    Path(params): Path<CrateDetailHandlerParams>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    let req_version = params.version.ok_or_else(|| {
        AxumNope::Redirect(
            format!("/crate/{}/{}", &params.name, ReqVersion::Latest),
            CachePolicy::ForeverInCdn,
        )
    })?;

    let matched_release = match_version(&mut conn, &params.name, &req_version)
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                format!("/crate/{}/{}", &params.name, version),
                CachePolicy::ForeverInCdn,
            )
        })?;

    let mut details = CrateDetails::from_matched_release(&mut conn, matched_release).await?;

    match details.fetch_readme(&storage).await {
        Ok(readme) => details.readme = readme.or(details.readme),
        Err(e) => warn!("error fetching readme: {:?}", &e),
    }

    let mut res = CrateDetailsPage {
        details,
        csp_nonce: String::new(),
    }
    .into_response();
    res.extensions_mut()
        .insert::<CachePolicy>(if req_version.is_latest() {
            CachePolicy::ForeverInCdn
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser
        });
    Ok(res.into_response())
}

#[derive(Template)]
#[template(path = "rustdoc/releases.html")]
#[derive(Debug, Clone, PartialEq, Serialize)]
struct ReleaseList {
    releases: Vec<Release>,
    crate_name: String,
    inner_path: String,
    target: String,
    csp_nonce: String,
}

impl_axum_webpage! {
    ReleaseList,
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

    let matched_release = match_version(&mut conn, &params.name, &params.version)
        .await?
        .into_canonical_req_version_or_else(|_| AxumNope::VersionNotFound)?;

    if matched_release.build_status() != BuildStatus::Success {
        // This handler should only be used for successful builds, so then we have all rows in the
        // `releases` table filled with data.
        // If we need this view at some point for in-progress releases or failed releases, we need
        // to handle empty doc targets.
        return Err(AxumNope::CrateNotFound);
    }

    let doc_targets = sqlx::query_scalar!(
        "SELECT
            releases.doc_targets
         FROM releases
         WHERE releases.id = $1;",
        matched_release.id(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::CrateNotFound)?
    .map(MetaData::parse_doc_targets)
    .ok_or_else(|| anyhow!("empty doc targets for successful release"))?;

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

    let target_name = matched_release
        .target_name()
        .ok_or_else(|| anyhow!("empty target name for succesful release"))?;

    let inner_path = if inner_path.is_empty() {
        format!("{}/index.html", target_name)
    } else {
        format!("{}/{inner_path}", target_name)
    };

    let target = if target.is_empty() {
        String::new()
    } else {
        format!("{target}/")
    };

    let res = ReleaseList {
        releases: matched_release.all_releases,
        target,
        inner_path,
        crate_name: params.name,
        csp_nonce: String::new(),
    };
    Ok(res.into_response())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ShortMetadata {
    name: String,
    version: Version,
    req_version: ReqVersion,
    doc_targets: Vec<String>,
}

impl ShortMetadata {
    // Used in templates.
    pub(crate) fn doc_targets(&self) -> Option<&[String]> {
        Some(&self.doc_targets)
    }
}

#[derive(Template)]
#[template(path = "rustdoc/platforms.html")]
#[derive(Debug, Clone, PartialEq, Serialize)]
struct PlatformList {
    metadata: ShortMetadata,
    inner_path: String,
    use_direct_platform_links: bool,
    current_target: String,
    csp_nonce: String,
}

impl_axum_webpage! {
    PlatformList,
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

    let matched_release = match_version(&mut conn, &params.name, &params.version)
        .await?
        .into_exactly_named_or_else(|corrected_name, req_version| {
            AxumNope::Redirect(
                encode_url_path(&format!(
                    "/platforms/{}/{}/{}",
                    corrected_name,
                    req_version,
                    req_path.join("/")
                )),
                CachePolicy::NoCaching,
            )
        })?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                encode_url_path(&format!(
                    "/platforms/{}/{}/{}",
                    &params.name,
                    version,
                    req_path.join("/")
                )),
                CachePolicy::ForeverInCdn,
            )
        })?;

    let krate = sqlx::query!(
        "SELECT
            releases.default_target,
            releases.doc_targets
        FROM releases
        WHERE releases.id = $1;",
        matched_release.id(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AxumNope::CrateNotFound)?;

    if krate.doc_targets.is_none()
        || krate.default_target.is_none()
        || matched_release.target_name().is_none()
    {
        // when the build wasn't finished, we don't have any target platforms
        // we could read from.
        return Ok(PlatformList {
            metadata: ShortMetadata {
                name: params.name,
                version: matched_release.version().clone(),
                req_version: params.version.clone(),
                doc_targets: Vec::new(),
            },
            inner_path: "".into(),
            use_direct_platform_links: is_crate_root,
            current_target: "".into(),
            csp_nonce: String::new(),
        }
        .into_response());
    }

    let doc_targets = MetaData::parse_doc_targets(krate.doc_targets.unwrap());

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
        format!("{}/index.html", matched_release.target_name().unwrap())
    } else {
        format!("{}/{inner_path}", matched_release.target_name().unwrap())
    };

    let latest_release = latest_release(&matched_release.all_releases)
        .expect("we couldn't end up here without releases");

    let current_target = if latest_release.build_status.is_success() {
        if target.is_empty() {
            krate.default_target.unwrap()
        } else {
            target.to_owned()
        }
    } else {
        String::new()
    };

    Ok(PlatformList {
        metadata: ShortMetadata {
            name: params.name,
            version: matched_release.version().clone(),
            req_version: params.version.clone(),
            doc_targets,
        },
        inner_path,
        use_direct_platform_links: is_crate_root,
        current_target,
        csp_nonce: String::new(),
    }
    .into_response())
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
    use crate::test::{
        assert_cache_control, assert_redirect, assert_redirect_cached, async_wrapper,
        fake_release_that_failed_before_build, wrapper, FakeBuild, TestDatabase, TestEnvironment,
    };
    use crate::{db::update_build_status, registry_api::CrateOwner};
    use anyhow::Error;
    use kuchikiki::traits::TendrilSink;
    use pretty_assertions::assert_eq;
    use reqwest::StatusCode;
    use std::collections::HashMap;

    async fn release_build_status(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &str,
    ) -> BuildStatus {
        let status = sqlx::query_scalar!(
            r#"
            SELECT build_status as "build_status!: BuildStatus"
            FROM crates
            INNER JOIN releases ON crates.id = releases.crate_id
            INNER JOIN release_build_status ON releases.id = release_build_status.rid
            WHERE crates.name = $1 AND releases.version = $2"#,
            name,
            version
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();

        assert_eq!(
            crate_details(&mut *conn, name, version, None)
                .await
                .build_status,
            status
        );

        status
    }

    async fn crate_details(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &str,
        req_version: Option<ReqVersion>,
    ) -> CrateDetails {
        let crate_id: i32 = sqlx::query_scalar!("SELECT id FROM crates WHERE name = $1", name)
            .fetch_one(&mut *conn)
            .await
            .unwrap();

        let releases = releases_for_crate(&mut *conn, crate_id).await.unwrap();

        CrateDetails::new(
            &mut *conn,
            name,
            &Version::parse(version).unwrap(),
            req_version,
            releases,
        )
        .await
        .unwrap()
        .unwrap()
    }

    #[fn_error_context::context("assert_last_successful_build_equals({package}, {version}, {expected_last_successful_build:?})")]
    async fn assert_last_successful_build_equals(
        db: &TestDatabase,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<&str>,
    ) -> Result<(), Error> {
        let mut conn = db.async_conn().await;
        let details = crate_details(&mut conn, package, version, None).await;

        anyhow::ensure!(
            details.last_successful_build.as_deref() == expected_last_successful_build,
            "didn't expect {:?}",
            details.last_successful_build,
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
                crate_details(&mut conn, "foo", "0.2.0", None).await
            });

            assert_eq!(
                details.releases,
                vec![
                    Release {
                        version: semver::Version::parse("1.0.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[0].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.12.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[1].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.3.0")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(false),
                        id: details.releases[2].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.2.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(true),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[3].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.2.0-alpha")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[4].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.1.1")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[5].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.1.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[6].id,
                        target_name: Some("foo".to_owned()),
                    },
                    Release {
                        version: semver::Version::parse("0.0.1")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(false),
                        rustdoc_status: Some(false),
                        id: details.releases[7].id,
                        target_name: Some("foo".to_owned()),
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
                    crate_details(&mut conn, "foo", version, None).await
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
                    crate_details(&mut conn, "foo", version, None).await
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
                    crate_details(&mut conn, "foo", version, None).await
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
                    crate_details(&mut conn, "foo", version, None).await
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
    fn test_latest_version_in_progress() {
        wrapper(|env| {
            let db = env.db();

            env.fake_release().name("foo").version("0.0.1").create()?;
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress)
                ])
                .create()?;

            for version in &["0.0.1", "0.0.2"] {
                let details = env.runtime().block_on(async move {
                    let mut conn = db.async_conn().await;
                    crate_details(&mut conn, "foo", version, None).await
                });
                assert_eq!(
                    details.latest_release().unwrap().version,
                    semver::Version::parse("0.0.1")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn releases_dropdowns_show_binary_warning() {
        wrapper(|env| {
            env.fake_release()
                .name("binary")
                .version("0.1.0")
                .binary(true)
                .create()?;

            let page = kuchikiki::parse_html()
                .one(env.frontend().get("/crate/binary/latest").send()?.text()?);
            let link = page
                .select_first("a.pure-menu-link[href='/crate/binary/0.1.0']")
                .unwrap();

            assert_eq!(
                link.as_node()
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
    fn releases_dropdowns_show_in_progress() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress)
                ])
                .create()?;

            let response = env.frontend().get("/crate/foo/latest").send()?;
            // FIXME: temporarily we don't show in-progress releases anywhere, which means we don't
            // show releases without builds anywhere.
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            // let page = kuchikiki::parse_html().one(response.text()?);
            // let link = page
            //     .select_first("a.pure-menu-link[href='/crate/foo/0.1.0']")
            //     .unwrap();

            // assert_eq!(
            //     link.as_node()
            //         .as_element()
            //         .unwrap()
            //         .attributes
            //         .borrow()
            //         .get("title")
            //         .unwrap(),
            //     "foo-0.1.0 is currently being built"
            // );

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
                    kind: OwnerKind::User,
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                crate_details(&mut conn, "foo", "0.0.1", None).await
            });
            assert_eq!(
                details.owners,
                vec![(
                    "foobar".into(),
                    "https://example.org/foobar".into(),
                    OwnerKind::User
                )]
            );

            // Adding a new owner, and changing details on an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.2")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobarv2".into(),
                    kind: OwnerKind::User,
                })
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                    kind: OwnerKind::User,
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                crate_details(&mut conn, "foo", "0.0.1", None).await
            });
            let mut owners = details.owners;
            owners.sort();
            assert_eq!(
                owners,
                vec![
                    (
                        "barfoo".into(),
                        "https://example.org/barfoo".into(),
                        OwnerKind::User
                    ),
                    (
                        "foobar".into(),
                        "https://example.org/foobarv2".into(),
                        OwnerKind::User
                    )
                ]
            );

            // Removing an existing owner
            env.fake_release()
                .name("foo")
                .version("0.0.3")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                    kind: OwnerKind::User,
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                crate_details(&mut conn, "foo", "0.0.1", None).await
            });
            assert_eq!(
                details.owners,
                vec![(
                    "barfoo".into(),
                    "https://example.org/barfoo".into(),
                    OwnerKind::User
                )]
            );

            // Changing owner details on another of their crates applies the change to both
            env.fake_release()
                .name("bar")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoov2".into(),
                    kind: OwnerKind::User,
                })
                .create()?;

            let details = env.runtime().block_on(async move {
                let mut conn = db.async_conn().await;
                crate_details(&mut conn, "foo", "0.0.1", None).await
            });
            assert_eq!(
                details.owners,
                vec![(
                    "barfoo".into(),
                    "https://example.org/barfoov2".into(),
                    OwnerKind::User
                )]
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
    fn test_minimal_failed_release_doesnt_error_features() {
        wrapper(|env| {
            env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors")
                    .await
            })?;

            let text_content = env
                .frontend()
                .get("/crate/foo/0.1.0/features")
                .send()?
                .error_for_status()?
                .text()?;

            assert!(text_content.contains(
                "Feature flags are not available for this release because \
                 the build failed before we could retrieve them"
            ));

            Ok(())
        });
    }

    #[test]
    fn test_minimal_failed_release_doesnt_error() {
        wrapper(|env| {
            env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors")
                    .await
            })?;

            let text_content = env
                .frontend()
                .get("/crate/foo/0.1.0")
                .send()?
                .error_for_status()?
                .text()?;

            assert!(text_content.contains("docs.rs failed to build foo"));

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
                    "url: {url:?}, ajax: {ajax:?}, should_contain_redirect: {should_contain_redirect:?}",
                );
                if !should_contain_redirect {
                    assert_eq!(rel, "");
                } else {
                    assert_eq!(rel, "nofollow");
                }
            }
            platform_links
        }

        fn run_check_links_redir(env: &TestEnvironment, url: &str, should_contain_redirect: bool) {
            let response = env.frontend().get(url).send().unwrap();
            assert!(response.status().is_success());
            let text = response.text().unwrap();
            let list1 = check_links(text.clone(), false, should_contain_redirect);

            // Same test with AJAX endpoint.
            let platform_menu_url = kuchikiki::parse_html()
                .one(text)
                .select_first("#platforms")
                .expect("invalid selector")
                .attributes
                .borrow()
                .get("data-url")
                .expect("data-url")
                .to_string();
            let response = env.frontend().get(&platform_menu_url).send().unwrap();
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

            run_check_links_redir(env, "/crate/dummy/0.4.0/features", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0/builds", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0/source/", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0/source/README.md", false);
            run_check_links_redir(env, "/crate/dummy/0.4.0", false);

            run_check_links_redir(env, "/dummy/latest/dummy", true);
            run_check_links_redir(env, "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy", true);
            run_check_links_redir(
                env,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.A.html",
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
                crate_details(&mut conn, "dummy", "0.5.0", None).await
            });
            assert!(matches!(
                env.runtime()
                    .block_on(details.fetch_readme(&env.runtime().block_on(env.async_storage()))),
                Ok(None)
            ));
            Ok(())
        });
    }

    #[test]
    fn test_crate_name_with_other_uri_chars() {
        wrapper(|env| {
            env.fake_release().name("dummy").version("1.0.0").create()?;

            assert_eq!(
                env.frontend()
                    .get_no_redirect("/crate/dummy%3E")
                    .send()?
                    .status(),
                StatusCode::FOUND
            );

            Ok(())
        })
    }

    #[test]
    fn test_build_status_no_builds() {
        async_wrapper(|env| async move {
            let release_id = env
                .async_fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .create_async()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;
            sqlx::query!("DELETE FROM builds")
                .execute(&mut *conn)
                .await?;

            update_build_status(&mut conn, release_id).await?;

            assert_eq!(
                release_build_status(&mut conn, "dummy", "0.1.0").await,
                BuildStatus::InProgress
            );

            Ok(())
        })
    }

    #[test]
    fn test_build_status_successful() {
        async_wrapper(|env| async move {
            env.async_fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::Success),
                    FakeBuild::default().build_status(BuildStatus::Failure),
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create_async()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;

            assert_eq!(
                release_build_status(&mut conn, "dummy", "0.1.0").await,
                BuildStatus::Success
            );

            Ok(())
        })
    }

    #[test]
    fn test_build_status_failed() {
        async_wrapper(|env| async move {
            env.async_fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::Failure),
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create_async()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;

            assert_eq!(
                release_build_status(&mut conn, "dummy", "0.1.0").await,
                BuildStatus::Failure
            );

            Ok(())
        })
    }

    #[test]
    fn test_build_status_in_progress() {
        async_wrapper(|env| async move {
            env.async_fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress)
                ])
                .create_async()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;

            assert_eq!(
                release_build_status(&mut conn, "dummy", "0.1.0").await,
                BuildStatus::InProgress
            );

            Ok(())
        })
    }
}
