use super::{MetaData, match_version};
use crate::db::{BuildId, ReleaseId};
use crate::registry_api::OwnerKind;
use crate::utils::{get_correct_docsrs_style_file, report_error};
use crate::{
    AsyncStorage,
    db::{CrateId, types::BuildStatus},
    impl_axum_webpage,
    storage::PathNotFoundError,
    web::{
        MatchedRelease, ReqVersion,
        cache::CachePolicy,
        error::{AxumNope, AxumResult, EscapedURI},
        extractors::{DbConnection, Path},
        page::templates::{RenderBrands, RenderRegular, RenderSolid, filters},
        rustdoc::RustdocHtmlParams,
    },
};
use anyhow::{Context, Result, anyhow};
use askama::Template;
use axum::{
    extract::Extension,
    response::{IntoResponse, Response as AxumResponse},
};
use chrono::{DateTime, Utc};
use futures_util::stream::TryStreamExt;
use log::warn;
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

// TODO: Add target name and versions
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CrateDetails {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) description: Option<String>,
    pub(crate) owners: Vec<(String, String, OwnerKind)>,
    pub(crate) dependencies: Option<Value>,
    readme: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    release_time: Option<DateTime<Utc>>,
    build_status: BuildStatus,
    pub latest_build_id: Option<BuildId>,
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
    pub(crate) parsed_license: Option<Vec<super::licenses::LicenseSegment>>,
    pub(crate) documentation_url: Option<String>,
    pub(crate) total_items: Option<i32>,
    pub(crate) documented_items: Option<i32>,
    pub(crate) total_items_needing_examples: Option<i32>,
    pub(crate) items_with_examples: Option<i32>,
    /// Database id for this crate
    pub(crate) crate_id: CrateId,
    /// Database id for this release
    pub(crate) release_id: ReleaseId,
    source_size: Option<i64>,
    documentation_size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
struct RepositoryMetadata {
    stars: i32,
    forks: i32,
    issues: i32,
    name: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Release {
    pub id: ReleaseId,
    pub version: semver::Version,
    #[allow(clippy::doc_overindented_list_items)]
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
    ///
    /// calculated in a database view : `release_build_status`
    pub build_status: BuildStatus,
    pub yanked: Option<bool>,
    pub is_library: Option<bool>,
    pub rustdoc_status: Option<bool>,
    pub target_name: Option<String>,
    pub release_time: Option<DateTime<Utc>>,
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
                crates.id AS "crate_id: CrateId",
                releases.id AS "release_id: ReleaseId",
                crates.name,
                releases.version,
                releases.description,
                releases.dependencies,
                releases.readme,
                releases.description_long,
                releases.release_time,
                release_build_status.build_status as "build_status!: BuildStatus",
                -- this is the latest build ID that generated content
                -- it's used to invalidate some blob storage related caches.
                builds.id as "latest_build_id?: BuildId",
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
                releases.source_size as "source_size?",
                builds.documentation_size as "documentation_size?",
                -- we're using the rustc version here to set the correct CSS file
                -- in the metadata.
                -- So we're only interested in successful builds here.
                builds.rustc_version as "rustc_version?",
                doc_coverage.total_items,
                doc_coverage.documented_items,
                doc_coverage.total_items_needing_examples,
                doc_coverage.items_with_examples
            FROM releases
            INNER JOIN release_build_status ON releases.id = release_build_status.rid
            INNER JOIN crates ON releases.crate_id = crates.id
            LEFT JOIN doc_coverage ON doc_coverage.release_id = releases.id
            LEFT JOIN repositories ON releases.repository_id = repositories.id
            LEFT JOIN LATERAL (
                 SELECT rustc_version, documentation_size, id
                 FROM builds
                 WHERE
                    builds.rid = releases.id AND
                    builds.build_status = 'success'
                 ORDER BY builds.build_finished
                 DESC LIMIT 1
             ) AS builds ON true
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

        // When documentation_url points to docs.rs itself, then we don't need to
        // show it on the page because user is already on docs.rs website
        let documentation_url = match krate.documentation_url {
            Some(url) if url.starts_with("https://docs.rs/") => None,
            Some(url) => Some(url),
            None => None,
        };

        let parsed_license = krate.license.as_deref().map(super::licenses::parse_license);

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
            documentation_url,
            is_library: krate.is_library,
            license: krate.license,
            parsed_license,
            documented_items: krate.documented_items,
            total_items: krate.total_items,
            total_items_needing_examples: krate.total_items_needing_examples,
            items_with_examples: krate.items_with_examples,
            crate_id: krate.crate_id,
            release_id: krate.release_id,
            documentation_size: krate.documentation_size,
            source_size: krate.source_size,
        };

        // get owners
        crate_details.owners = sqlx::query!(
            r#"SELECT login, avatar, kind as "kind: OwnerKind"
             FROM owners
             INNER JOIN owner_rels ON owner_rels.oid = owners.id
             WHERE cid = $1"#,
            krate.crate_id.0,
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
                self.latest_build_id,
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
            .parse::<toml::Table>()
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
                    self.latest_build_id,
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
    crate_id: CrateId,
) -> Result<Vec<Release>, anyhow::Error> {
    let mut releases: Vec<Release> = sqlx::query!(
        r#"SELECT
             releases.id as "id: ReleaseId",
             releases.version,
             release_build_status.build_status as "build_status!: BuildStatus",
             releases.yanked,
             releases.is_library,
             releases.rustdoc_status,
             releases.release_time,
             releases.target_name
         FROM releases
         INNER JOIN release_build_status ON releases.id = release_build_status.rid
         WHERE
             releases.crate_id = $1"#,
        crate_id.0,
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
            release_time: row.release_time,
        }))
    })
    .try_collect()
    .await?;

    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
}

#[derive(Template)]
#[template(path = "crate/details.html")]
#[derive(Debug, Clone, PartialEq)]
struct CrateDetailsPage {
    version: Version,
    name: String,
    owners: Vec<(String, String, OwnerKind)>,
    metadata: MetaData,
    documented_items: Option<i32>,
    total_items: Option<i32>,
    total_items_needing_examples: Option<i32>,
    items_with_examples: Option<i32>,
    homepage_url: Option<String>,
    documentation_url: Option<String>,
    repository_url: Option<String>,
    repository_metadata: Option<RepositoryMetadata>,
    dependencies: Option<Value>,
    releases: Vec<Release>,
    readme: Option<String>,
    build_status: BuildStatus,
    rustdoc_status: Option<bool>,
    is_library: Option<bool>,
    last_successful_build: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    source_size: Option<i64>,
    documentation_size: Option<i64>,
}

impl CrateDetailsPage {
    // Used by templates.
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
}

impl_axum_webpage! {
    CrateDetailsPage,
    cpu_intensive_rendering = true,
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
            EscapedURI::new(
                &format!("/crate/{}/{}", &params.name, ReqVersion::Latest),
                None,
            ),
            CachePolicy::ForeverInCdn,
        )
    })?;

    let matched_release = match_version(&mut conn, &params.name, &req_version)
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                EscapedURI::new(&format!("/crate/{}/{}", &params.name, version), None),
                CachePolicy::ForeverInCdn,
            )
        })?;

    let mut details = CrateDetails::from_matched_release(&mut conn, matched_release).await?;

    match details.fetch_readme(&storage).await {
        Ok(readme) => details.readme = readme.or(details.readme),
        Err(e) => warn!("error fetching readme: {:?}", &e),
    }

    let CrateDetails {
        version,
        name,
        owners,
        metadata,
        documented_items,
        total_items,
        total_items_needing_examples,
        items_with_examples,
        homepage_url,
        documentation_url,
        repository_url,
        repository_metadata,
        dependencies,
        releases,
        readme,
        build_status,
        rustdoc_status,
        is_library,
        last_successful_build,
        rustdoc,
        source_size,
        documentation_size,
        ..
    } = details;

    let mut res = CrateDetailsPage {
        version,
        name,
        owners,
        metadata,
        documented_items,
        total_items,
        total_items_needing_examples,
        items_with_examples,
        homepage_url,
        documentation_url,
        repository_url,
        repository_metadata,
        dependencies,
        releases,
        readme,
        build_status,
        rustdoc_status,
        is_library,
        last_successful_build,
        rustdoc,
        source_size,
        documentation_size,
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
#[derive(Debug, Clone, PartialEq)]
struct ReleaseList {
    releases: Vec<Release>,
    crate_name: String,
    inner_path: String,
    target: String,
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
        matched_release.id().0,
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
        format!("{target_name}/index.html")
    } else {
        format!("{target_name}/{inner_path}")
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
    };
    Ok(res.into_response())
}

#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
struct PlatformList {
    metadata: ShortMetadata,
    inner_path: String,
    use_direct_platform_links: bool,
    current_target: String,
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
                EscapedURI::new(
                    &format!(
                        "/platforms/{}/{}/{}",
                        corrected_name,
                        req_version,
                        req_path.join("/")
                    ),
                    None,
                ),
                CachePolicy::NoCaching,
            )
        })?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                EscapedURI::new(
                    &format!(
                        "/platforms/{}/{}/{}",
                        &params.name,
                        version,
                        req_path.join("/")
                    ),
                    None,
                ),
                CachePolicy::ForeverInCdn,
            )
        })?;

    let krate = sqlx::query!(
        "SELECT
            releases.default_target,
            releases.doc_targets
        FROM releases
        WHERE releases.id = $1;",
        matched_release.id().0,
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
        AxumResponseTestExt, AxumRouterTestExt, FakeBuild, TestDatabase, TestEnvironment,
        async_wrapper, fake_release_that_failed_before_build,
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
        let crate_id = sqlx::query_scalar!(
            r#"SELECT id as "id: CrateId" FROM crates WHERE name = $1"#,
            name
        )
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

    #[fn_error_context::context(
        "assert_last_successful_build_equals({package}, {version}, {expected_last_successful_build:?})"
    )]
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
    fn test_crate_details_documentation_url_is_none_when_url_is_docs_rs() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .documentation_url(Some("https://foo.com".into()))
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.2.0")
                .documentation_url(Some("https://docs.rs/foo/".into()))
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.3.0")
                .documentation_url(None)
                .create()
                .await?;

            let details_0_1 = crate_details(&mut conn, "foo", "0.1.0", None).await;
            let details_0_2 = crate_details(&mut conn, "foo", "0.2.0", None).await;
            let details_0_3 = crate_details(&mut conn, "foo", "0.3.0", None).await;

            assert_eq!(
                details_0_1.documentation_url,
                Some("https://foo.com".into())
            );
            assert_eq!(details_0_2.documentation_url, None);
            assert_eq!(details_0_3.documentation_url, None);

            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_when_last_releases_failed_or_yanked() {
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.4")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.5")
                .build_result_failed()
                .yanked(true)
                .create()
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
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()
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
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.4")
                .create()
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
        async_wrapper(|env| async move {
            let db = env.async_db();

            // Add new releases of 'foo' out-of-order since CrateDetails should sort them descending
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.3.0")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("1.0.0")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.12.0")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.2.0")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.2.0-alpha")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .build_result_failed()
                .binary(true)
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            let mut details = crate_details(&mut conn, "foo", "0.2.0", None).await;
            for detail in &mut details.releases {
                detail.release_time = None;
            }

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
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.12.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[1].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.3.0")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(false),
                        id: details.releases[2].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.2.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(true),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[3].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.2.0-alpha")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[4].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.1.1")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[5].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.1.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[6].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                    Release {
                        version: semver::Version::parse("0.0.1")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(false),
                        rustdoc_status: Some(false),
                        id: details.releases[7].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                    },
                ]
            );

            Ok(())
        });
    }

    #[test]
    fn test_canonical_url() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create()
                .await?;

            let response = env.web_app().await.get("/crate/foo/0.0.1").await?;
            response
                .assert_cache_control(CachePolicy::ForeverInCdnAndStaleInBrowser, &env.config());

            assert!(
                response
                    .text()
                    .await?
                    .contains("rel=\"canonical\" href=\"https://docs.rs/crate/foo/latest")
            );

            Ok(())
        })
    }

    #[test]
    fn test_latest_version() {
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
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
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3-pre.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            for version in &["0.0.1", "0.0.2", "0.0.3-pre.1"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
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
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
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
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.3")
                .yanked(true)
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .yanked(true)
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
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
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("foo")
                .version("0.0.2")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            for version in &["0.0.1", "0.0.2"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("binary")
                .version("0.1.0")
                .binary(true)
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/binary/latest")
                    .await?
                    .text()
                    .await?,
            );
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            let response = env.web_app().await.get("/crate/foo/latest").await?;

            let page = kuchikiki::parse_html().one(response.text().await?);
            let link = page
                .select_first("a.pure-menu-link[href='/crate/foo/0.1.0']")
                .unwrap();

            assert_eq!(
                link.as_node()
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow()
                    .get("title")
                    .unwrap(),
                "foo-0.1.0 is currently being built"
            );

            Ok(())
        });
    }

    #[test]
    fn test_updating_owners() {
        async_wrapper(|env| async move {
            let db = env.async_db();

            env.fake_release()
                .await
                .name("foo")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "foobar".into(),
                    avatar: "https://example.org/foobar".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            let details = crate_details(&mut conn, "foo", "0.0.1", None).await;
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
                .await
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
                .create()
                .await?;

            let details = crate_details(&mut conn, "foo", "0.0.1", None).await;
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
                .await
                .name("foo")
                .version("0.0.3")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoo".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            let details = crate_details(&mut conn, "foo", "0.0.1", None).await;
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
                .await
                .name("bar")
                .version("0.0.1")
                .add_owner(CrateOwner {
                    login: "barfoo".into(),
                    avatar: "https://example.org/barfoov2".into(),
                    kind: OwnerKind::User,
                })
                .create()
                .await?;

            let mut conn = db.async_conn().await;
            let details = crate_details(&mut conn, "foo", "0.0.1", None).await;
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .features(HashMap::new())
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/library/0.1.0/features")
                    .await?
                    .text()
                    .await?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn feature_private_feature_flags_are_hidden() {
        async_wrapper(|env| async move {
            let features = [("_private".into(), Vec::new())]
                .iter()
                .cloned()
                .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/library/0.1.0/features")
                    .await?
                    .text()
                    .await?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn feature_flags_without_default() {
        async_wrapper(|env| async move {
            let features = [("feature1".into(), Vec::new())]
                .iter()
                .cloned()
                .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/library/0.1.0/features")
                    .await?
                    .text()
                    .await?,
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
        async_wrapper(|env| async move {
            let features = [
                ("default".into(), vec!["feature1".into()]),
                ("feature1".into(), vec!["feature2".into()]),
                ("feature2".into(), Vec::new()),
            ]
            .iter()
            .cloned()
            .collect::<HashMap<String, Vec<String>>>();
            env.fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .features(features)
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/library/0.1.0/features")
                    .await?
                    .text()
                    .await?,
            );
            assert!(page.select_first(r#"p[data-id="empty-features"]"#).is_err());
            let def_len = page
                .select_first(r#"b[data-id="default-feature-len"]"#)
                .unwrap();
            assert_eq!(def_len.text_contents(), "2");
            Ok(())
        });
    }

    #[test]
    fn details_with_repository_and_stats_can_render_icon() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .repo("https://github.com/org/repo")
                .github_stats("org/repo", 10, 10, 10)
                .create()
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .assert_success("/crate/library/0.1.0")
                    .await?
                    .text()
                    .await?,
            );

            let link = page
                .select_first("a.pure-menu-link[href='https://github.com/org/repo']")
                .unwrap();

            let icon_node = link.as_node().children().nth(1).unwrap();
            assert_eq!(
                icon_node
                    .as_element()
                    .unwrap()
                    .attributes
                    .borrow()
                    .get("class")
                    .unwrap(),
                "fa fa-solid fa-code-branch "
            );

            Ok(())
        });
    }

    #[test]
    fn feature_flags_report_null() {
        async_wrapper(|env| async move {
            let id = env
                .fake_release()
                .await
                .name("library")
                .version("0.1.0")
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;
            sqlx::query!("UPDATE releases SET features = NULL WHERE id = $1", id.0)
                .execute(&mut *conn)
                .await?;

            let page = kuchikiki::parse_html().one(
                env.web_app()
                    .await
                    .get("/crate/library/0.1.0/features")
                    .await?
                    .text()
                    .await?,
            );
            assert!(page.select_first(r#"p[data-id="null-features"]"#).is_ok());
            Ok(())
        });
    }

    #[test]
    fn test_minimal_failed_release_doesnt_error_features() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().async_conn().await;
            fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors").await?;

            let text_content = env
                .web_app()
                .await
                .get("/crate/foo/0.1.0/features")
                .await?
                .error_for_status()?
                .text()
                .await?;

            assert!(text_content.contains(
                "Feature flags are not available for this release because \
                 the build failed before we could retrieve them"
            ));

            Ok(())
        });
    }

    #[test]
    fn test_minimal_failed_release_doesnt_error() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().async_conn().await;
            fake_release_that_failed_before_build(&mut conn, "foo", "0.1.0", "some errors").await?;

            let text_content = env
                .web_app()
                .await
                .get("/crate/foo/0.1.0")
                .await?
                .error_for_status()?
                .text()
                .await?;

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

        async fn run_check_links_redir(
            env: &TestEnvironment,
            url: &str,
            should_contain_redirect: bool,
        ) {
            let response = env.web_app().await.get(url).await.unwrap();
            let status = response.status();
            assert!(
                status.is_success(),
                "no success, status: {}, url: {}, target: {}",
                status,
                url,
                response.redirect_target().unwrap_or_default(),
            );
            let text = response.text().await.unwrap();
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
            let response = env.web_app().await.get(&platform_menu_url).await.unwrap();
            assert!(response.status().is_success());
            response.assert_cache_control(CachePolicy::ForeverInCdn, &env.config());
            let list2 = check_links(
                response.text().await.unwrap(),
                true,
                should_contain_redirect,
            );
            assert_eq!(list1, list2);
        }

        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/struct.A.html")
                .default_target("x86_64-unknown-linux-gnu")
                .add_target("x86_64-pc-windows-msvc")
                .source_file("README.md", b"storage readme")
                .create()
                .await?;

            run_check_links_redir(&env, "/crate/dummy/0.4.0/features", false).await;
            run_check_links_redir(&env, "/crate/dummy/0.4.0/builds", false).await;
            run_check_links_redir(&env, "/crate/dummy/0.4.0/source/", false).await;
            run_check_links_redir(&env, "/crate/dummy/0.4.0/source/README.md", false).await;
            run_check_links_redir(&env, "/crate/dummy/0.4.0", false).await;

            run_check_links_redir(&env, "/dummy/latest/dummy/", true).await;
            run_check_links_redir(
                &env,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/index.html",
                true,
            )
            .await;
            run_check_links_redir(
                &env,
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/struct.A.html",
                true,
            )
            .await;

            Ok(())
        });
    }

    #[test]
    fn check_crate_name_in_redirect() {
        async fn check_links(env: &TestEnvironment, url: &str, links: Vec<String>) {
            let response = env.web_app().await.get(url).await.unwrap();
            assert!(response.status().is_success());

            let platform_links: Vec<String> = kuchikiki::parse_html()
                .one(response.text().await.unwrap())
                .select("li a")
                .expect("invalid selector")
                .map(|el| {
                    let attributes = el.attributes.borrow();
                    attributes.get("href").expect("href").to_string()
                })
                .collect();

            assert_eq!(platform_links, links,);
        }

        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy-ba")
                .version("0.4.0")
                .rustdoc_file("dummy-ba/index.html")
                .rustdoc_file("x86_64-unknown-linux-gnu/dummy-ba/index.html")
                .add_target("x86_64-unknown-linux-gnu")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("dummy-ba")
                .version("0.5.0")
                .rustdoc_file("dummy-ba/index.html")
                .rustdoc_file("x86_64-unknown-linux-gnu/dummy-ba/index.html")
                .add_target("x86_64-unknown-linux-gnu")
                .create()
                .await?;

            check_links(
                &env,
                "/crate/dummy-ba/latest/menus/releases/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/dummy_ba/index.html".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/dummy_ba/index.html".to_string(),
                ],
            )
            .await;

            check_links(
                &env,
                "/crate/dummy-ba/latest/menus/releases/x86_64-unknown-linux-gnu/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/index.html".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/index.html".to_string(),
                ],
            ).await;

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
            async_wrapper(|env| async move {
                let mut rel = env
                    .fake_release()
                    .await
                    .name("dummy")
                    .version("0.4.0")
                    .rustdoc_file("dummy/index.html")
                    .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                    .default_target("x86_64-unknown-linux-gnu");

                for nb in 0..nb_targets - 1 {
                    rel = rel.add_target(&format!("x86_64-pc-windows-msvc{nb}"));
                }
                rel.create().await?;

                let response = env.web_app().await.get("/crate/dummy/0.4.0").await?;
                assert!(response.status().is_success());

                let nb_li = kuchikiki::parse_html()
                    .one(response.text().await?)
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
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("dummy/index.html")
                .rustdoc_file("x86_64-pc-windows-msvc/dummy/index.html")
                .default_target("x86_64-unknown-linux-gnu")
                .add_target("x86_64-pc-windows-msvc")
                .create()
                .await?;
            let web = env.web_app().await;

            let resp = web.get("/crate/dummy/latest").await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(CachePolicy::ForeverInCdn, &env.config());
            let body = resp.text().await?;
            assert!(body.contains("<a href=\"/crate/dummy/latest/features\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/dummy/latest\""));

            web.assert_redirect("/crate/dummy/latest/", "/crate/dummy/latest")
                .await?;
            web.assert_redirect_cached(
                "/crate/dummy",
                "/crate/dummy/latest",
                CachePolicy::ForeverInCdn,
                &env.config(),
            )
            .await?;

            Ok(())
        });
    }

    #[test]
    fn readme() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .readme_only_database("database readme")
                .create()
                .await?;

            env.fake_release()
                .await
                .name("dummy")
                .version("0.2.0")
                .readme_only_database("database readme")
                .source_file("README.md", b"storage readme")
                .create()
                .await?;

            env.fake_release()
                .await
                .name("dummy")
                .version("0.3.0")
                .source_file("README.md", b"storage readme")
                .create()
                .await?;

            env.fake_release()
                .await
                .name("dummy")
                .version("0.4.0")
                .readme_only_database("database readme")
                .source_file("MEREAD", b"storage meread")
                .source_file("Cargo.toml", br#"package.readme = "MEREAD""#)
                .create()
                .await?;

            env.fake_release()
                .await
                .name("dummy")
                .version("0.5.0")
                .readme_only_database("database readme")
                .source_file("README.md", b"storage readme")
                .no_cargo_toml()
                .create()
                .await?;

            let check_readme = |path: String, content: String| {
                let env = env.clone();
                async move {
                    let resp = env.web_app().await.get(&path).await.unwrap();
                    let body = resp.text().await.unwrap();
                    assert!(body.contains(&content));
                }
            };

            check_readme("/crate/dummy/0.1.0".into(), "database readme".into()).await;
            check_readme("/crate/dummy/0.2.0".into(), "storage readme".into()).await;
            check_readme("/crate/dummy/0.3.0".into(), "storage readme".into()).await;
            check_readme("/crate/dummy/0.4.0".into(), "storage meread".into()).await;

            let mut conn = env.async_db().async_conn().await;
            let details = crate_details(&mut conn, "dummy", "0.5.0", None).await;
            assert!(matches!(
                details.fetch_readme(&env.async_storage()).await,
                Ok(None)
            ));
            Ok(())
        });
    }

    #[test]
    fn no_readme() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("0.2.0")
                .source_file(
                    "Cargo.toml",
                    br#"[package]
name = "dummy"
version = "0.2.0"

[lib]
name = "dummy"
path = "src/lib.rs"
"#,
                )
                .source_file(
                    "src/lib.rs",
                    b"//! # Crate-level docs
//!
//! ```
//! let x = 21;
//! ```
",
                )
                .target_source("src/lib.rs")
                .create()
                .await?;

            let web = env.web_app().await;
            let response = web.get("/crate/dummy/0.2.0").await?;
            assert!(response.status().is_success());

            let dom = kuchikiki::parse_html().one(response.text().await?);
            dom.select_first("#main").expect("not main crate docs");
            // First we check that the crate-level docs have been rendered as expected.
            assert_eq!(
                dom.select_first("#main h1")
                    .expect("no h1 found")
                    .text_contents(),
                "Crate-level docs"
            );
            // Then we check that by default, the language used for highlighting is rust.
            assert_eq!(
                dom.select_first("#main pre .syntax-source.syntax-rust")
                    .expect("no rust code block found")
                    .text_contents(),
                "let x = 21;\n"
            );
            Ok(())
        });
    }

    #[test]
    fn test_crate_name_with_other_uri_chars() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("1.0.0")
                .create()
                .await?;

            assert_eq!(
                env.web_app().await.get("/crate/dummy%3E").await?.status(),
                StatusCode::FOUND
            );

            Ok(())
        })
    }

    #[test]
    fn test_build_status_no_builds() {
        async_wrapper(|env| async move {
            let release_id = env
                .fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;
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
            env.fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::Success),
                    FakeBuild::default().build_status(BuildStatus::Failure),
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;

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
            env.fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::Failure),
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;

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
            env.fake_release()
                .await
                .name("dummy")
                .version("0.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            let mut conn = env.async_db().async_conn().await;

            assert_eq!(
                release_build_status(&mut conn, "dummy", "0.1.0").await,
                BuildStatus::InProgress
            );

            Ok(())
        })
    }

    #[test]
    fn test_sizes_display() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("dummy")
                .version("0.4.0")
                .rustdoc_file("dummy/index.html")
                .create()
                .await?;

            let web = env.web_app().await;
            let response = web.get("/crate/dummy/0.4.0").await?;
            assert!(response.status().is_success());

            let mut has_source_code_size = false;
            let mut has_doc_size = false;
            for span in kuchikiki::parse_html()
                .one(response.text().await?)
                .select(r#".pure-menu-item span.documented-info"#)
                .expect("invalid selector")
            {
                if span.text_contents().starts_with("Source code size:") {
                    has_source_code_size = true;
                } else if span.text_contents().starts_with("Documentation size:") {
                    has_doc_size = true;
                }
            }
            assert!(has_source_code_size);
            assert!(has_doc_size);
            Ok(())
        });
    }
}
