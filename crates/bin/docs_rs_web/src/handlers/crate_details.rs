use crate::{
    cache::CachePolicy,
    error::{AxumNope, AxumResult},
    extractors::{
        DbConnection,
        rustdoc::{PageKind, RustdocParams},
    },
    impl_axum_webpage,
    match_release::{MatchedRelease, match_version},
    metadata::MetaData,
    page::templates::{RenderBrands, RenderRegular, RenderSolid, filters},
    utils::{get_correct_docsrs_style_file, licenses},
};
use anyhow::{Context, Result, anyhow};
use askama::Template;
use axum::{
    extract::Extension,
    response::{IntoResponse, Response as AxumResponse},
};
use chrono::{DateTime, Utc};
use docs_rs_cargo_metadata::{Dependency, ReleaseDependencyList};
use docs_rs_database::crate_details::{Release, latest_release, parse_doc_targets};
use docs_rs_headers::CanonicalUrl;
use docs_rs_registry_api::OwnerKind;
use docs_rs_storage::{AsyncStorage, PathNotFoundError};
use docs_rs_types::{
    BuildId, BuildStatus, CrateId, Duration, KrateName, ReleaseId, ReqVersion, Version,
};
use futures_util::stream::TryStreamExt;
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

// TODO: Add target name and versions
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CrateDetails {
    pub(crate) name: KrateName,
    pub(crate) version: Version,
    pub(crate) description: Option<String>,
    pub(crate) owners: Vec<(String, String, OwnerKind)>,
    pub(crate) dependencies: Vec<Dependency>,
    readme: Option<String>,
    rustdoc: Option<String>, // this is description_long in database
    release_time: Option<DateTime<Utc>>,
    build_status: BuildStatus,
    pub latest_build_id: Option<BuildId>,
    last_successful_build: Option<Version>,
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
    pub(crate) parsed_license: Option<Vec<licenses::LicenseSegment>>,
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
        name: &KrateName,
        version: &Version,
        req_version: Option<ReqVersion>,
        prefetched_releases: Vec<Release>,
    ) -> Result<Option<CrateDetails>, anyhow::Error> {
        let krate = match sqlx::query!(
            r#"SELECT
                crates.id AS "crate_id: CrateId",
                releases.id AS "release_id: ReleaseId",
                crates.name as "name: KrateName",
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
            name as _,
            version as _,
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
            doc_targets: krate.doc_targets.map(parse_doc_targets),
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

        let parsed_license = krate.license.as_deref().map(licenses::parse_license);

        let dependencies: Vec<Dependency> = krate
            .dependencies
            .map(serde_json::from_value::<ReleaseDependencyList>)
            .transpose()
            // NOTE: we sometimes have invalid semver-requirement strings the database
            // (at the time writing, 14 releases out of 2 million).
            // We silently ignore those here.
            .unwrap_or_default()
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();

        let mut crate_details = CrateDetails {
            name: krate.name,
            version: version.clone(),
            description: krate.description,
            owners: Vec::new(),
            dependencies,
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
                .map(|release| release.version.clone())
                .next();
        }

        Ok(Some(crate_details))
    }

    async fn fetch_readme(&self, storage: &AsyncStorage) -> anyhow::Result<Option<String>> {
        let manifest = match storage
            .fetch_source_file(
                &self.name,
                &self.version,
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
                    &self.version,
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

#[derive(Debug, Clone, Default)]
struct BuildStatistics {
    avg_build_duration_release: Option<Duration>,
    avg_build_duration_crate: Option<Duration>,
}

impl BuildStatistics {
    fn has_data(&self) -> bool {
        self.avg_build_duration_crate.is_some() || self.avg_build_duration_release.is_some()
    }

    async fn fetch_for_release(
        conn: &mut sqlx::PgConnection,
        crate_id: CrateId,
        release_id: ReleaseId,
    ) -> Result<Self> {
        Ok(Self {
            avg_build_duration_release: sqlx::query_scalar!(
                r#"
                SELECT AVG(b.build_finished - b.build_started) AS "duration?: Duration"
                FROM builds AS b
                WHERE
                    b.rid = $1 AND
                    b.build_status = 'success' AND
                    b.build_started IS NOT NULL"#,
                release_id as _,
            )
            .fetch_optional(&mut *conn)
            .await?
            .flatten(),
            avg_build_duration_crate: sqlx::query_scalar!(
                r#"
                SELECT
                    AVG(b.build_finished - b.build_started) AS "duration?: Duration"

                FROM
                    crates AS c
                    INNER JOIN releases AS r on c.id = r.crate_id
                    INNER JOIN builds AS b on r.id = b.rid

                WHERE
                    c.id = $1 AND
                    b.build_status = 'success' AND
                    b.build_started IS NOT NULL"#,
                crate_id as _,
            )
            .fetch_optional(&mut *conn)
            .await?
            .flatten(),
        })
    }
}

#[derive(Debug, Clone, Template)]
#[template(path = "crate/details.html")]
struct CrateDetailsPage {
    version: Version,
    name: KrateName,
    owners: Vec<(String, String, OwnerKind)>,
    metadata: MetaData,
    documented_items: Option<i32>,
    total_items: Option<i32>,
    total_items_needing_examples: Option<i32>,
    build_statistics: BuildStatistics,
    items_with_examples: Option<i32>,
    homepage_url: Option<String>,
    documentation_url: Option<String>,
    repository_url: Option<String>,
    repository_metadata: Option<RepositoryMetadata>,
    dependencies: Vec<Dependency>,
    releases: Vec<Release>,
    readme: Option<String>,
    build_status: BuildStatus,
    rustdoc_status: Option<bool>,
    is_library: Option<bool>,
    last_successful_build: Option<Version>,
    rustdoc: Option<String>, // this is description_long in database
    source_size: Option<i64>,
    documentation_size: Option<i64>,
    canonical_url: CanonicalUrl,
    params: RustdocParams,
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

#[tracing::instrument(skip(conn, storage))]
pub(crate) async fn crate_details_handler(
    params: RustdocParams,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    let matched_release = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|confirmed_name, version| {
            let params = params
                .clone()
                .with_name(confirmed_name)
                .with_req_version(version);
            AxumNope::Redirect(
                params.crate_details_url(),
                CachePolicy::ForeverInCdn(confirmed_name.into()),
            )
        })?;
    let params = params.apply_matched_release(&matched_release);

    if params.original_path() != params.crate_details_url().path() {
        return Err(AxumNope::Redirect(
            params.crate_details_url(),
            CachePolicy::ForeverInCdn(matched_release.name.into()),
        ));
    }

    let mut details = CrateDetails::from_matched_release(&mut conn, matched_release).await?;

    match details.fetch_readme(&storage).await {
        Ok(readme) => details.readme = readme.or(details.readme),
        Err(e) => warn!(?e, "error fetching readme"),
    }

    let build_statistics =
        BuildStatistics::fetch_for_release(&mut conn, details.crate_id, details.release_id).await?;

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

    let is_latest_version = params.req_version().is_latest();

    let mut res = CrateDetailsPage {
        version,
        name: name.clone(),
        owners,
        metadata,
        documented_items,
        total_items,
        total_items_needing_examples,
        build_statistics,
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
        canonical_url: CanonicalUrl::from_uri(
            params
                .clone()
                .with_req_version(ReqVersion::Latest)
                .crate_details_url(),
        ),
        params,
    }
    .into_response();
    res.extensions_mut()
        .insert::<CachePolicy>(if is_latest_version {
            CachePolicy::ForeverInCdn(name.into())
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser(name.into())
        });
    Ok(res)
}

#[derive(Template)]
#[template(path = "rustdoc/releases.html")]
#[derive(Debug, Clone, PartialEq)]
struct ReleaseList {
    crate_name: KrateName,
    releases: Vec<Release>,
    params: RustdocParams,
}

impl_axum_webpage! {
    ReleaseList,
    cache_policy = |page| CachePolicy::ForeverInCdn(
        page.crate_name.clone().into()
    ),
    cpu_intensive_rendering = true,
}

#[tracing::instrument]
pub(crate) async fn get_all_releases(
    params: RustdocParams,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    let params = params.with_page_kind(PageKind::Rustdoc);
    // NOTE: we're getting RustDocParams here, where both target and path are optional.
    let matched_release = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .into_canonical_req_version_or_else(|_, _| AxumNope::VersionNotFound)?;
    let params = params.apply_matched_release(&matched_release);

    if matched_release.build_status() != BuildStatus::Success {
        // This handler should only be used for successful builds, so then we have all rows in the
        // `releases` table filled with data.
        // If we need this view at some point for in-progress releases or failed releases, we need
        // to handle empty doc targets.
        return Err(AxumNope::CrateNotFound);
    }

    Ok(ReleaseList {
        crate_name: matched_release.name.clone(),
        releases: matched_release.all_releases,
        params,
    }
    .into_response())
}

#[derive(Template)]
#[template(path = "rustdoc/platforms.html")]
#[derive(Debug, Clone, PartialEq)]
struct PlatformList {
    crate_name: KrateName,
    use_direct_platform_links: bool,
    current_target: String,
    params: RustdocParams,
}

impl_axum_webpage! {
    PlatformList,
    cache_policy = |page| CachePolicy::ForeverInCdn(
        page.crate_name.clone().into()
    ),
    cpu_intensive_rendering = true,
}

#[tracing::instrument]
pub(crate) async fn get_all_platforms_inner(
    mut params: RustdocParams,
    mut conn: DbConnection,
    is_crate_root: bool,
) -> AxumResult<AxumResponse> {
    if !is_crate_root {
        params = params.with_page_kind(PageKind::Rustdoc);
    }

    let matched_release = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .into_exactly_named_or_else(|corrected_name, req_version| {
            AxumNope::Redirect(
                params
                    .clone()
                    .with_name(corrected_name)
                    .with_req_version(req_version)
                    .platforms_partial_url(),
                CachePolicy::NoCaching,
            )
        })?
        .into_canonical_req_version_or_else(|confirmed_name, version| {
            let params = params
                .clone()
                .with_name(confirmed_name)
                .with_req_version(version);
            AxumNope::Redirect(
                params.platforms_partial_url(),
                CachePolicy::ForeverInCdn(confirmed_name.into()),
            )
        })?;
    let params = params.apply_matched_release(&matched_release);

    if !matched_release.build_status().is_success() {
        // when the build wasn't finished, we don't have any target platforms
        // we could read from.
        return Ok(PlatformList {
            crate_name: matched_release.name.clone(),
            use_direct_platform_links: is_crate_root,
            current_target: "".into(),
            params,
        }
        .into_response());
    }

    let latest_release = latest_release(&matched_release.all_releases)
        .expect("we couldn't end up here without releases");

    let current_target = if latest_release.build_status.is_success() {
        params
            .doc_target_or_default()
            .unwrap_or_default()
            .to_owned()
    } else {
        String::new()
    };

    Ok(PlatformList {
        crate_name: matched_release.name.clone(),
        use_direct_platform_links: is_crate_root,
        current_target,
        params,
    }
    .into_response())
}

pub(crate) async fn get_all_platforms_root(
    params: RustdocParams,
    conn: DbConnection,
) -> AxumResult<AxumResponse> {
    get_all_platforms_inner(params.with_inner_path(""), conn, true).await
}

pub(crate) async fn get_all_platforms(
    params: RustdocParams,
    conn: DbConnection,
) -> AxumResult<AxumResponse> {
    get_all_platforms_inner(params, conn, false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
        async_wrapper,
    };
    use anyhow::Error;
    use docs_rs_database::Pool;
    use docs_rs_database::{crate_details::releases_for_crate, releases::update_build_status};
    use docs_rs_registry_api::CrateOwner;
    use docs_rs_test_fakes::{FakeBuild, fake_release_that_failed_before_build};
    use docs_rs_types::KrateName;
    use docs_rs_types::testing::{FOO, V1};
    use http::StatusCode;
    use kuchikiki::traits::TendrilSink;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use std::str::FromStr as _;
    use test_case::test_case;

    async fn release_build_status(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &str,
    ) -> BuildStatus {
        let version: Version = version.parse().expect("invalid version");

        let status = sqlx::query_scalar!(
            r#"
            SELECT build_status as "build_status!: BuildStatus"
            FROM crates
            INNER JOIN releases ON crates.id = releases.crate_id
            INNER JOIN release_build_status ON releases.id = release_build_status.rid
            WHERE crates.name = $1 AND releases.version = $2"#,
            name,
            version as _
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

    async fn crate_details<K, V>(
        conn: &mut sqlx::PgConnection,
        name: K,
        version: V,
        req_version: Option<ReqVersion>,
    ) -> CrateDetails
    where
        K: TryInto<KrateName>,
        K::Error: std::error::Error + Send + Sync + 'static,
        V: TryInto<Version>,
        V::Error: std::error::Error + Send + Sync + 'static,
    {
        let name = name.try_into().expect("invalid crate name");
        let version = version.try_into().expect("invalid version");

        let crate_id = sqlx::query_scalar!(
            r#"SELECT id as "id: CrateId" FROM crates WHERE name = $1"#,
            name as _
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();

        let releases = releases_for_crate(&mut *conn, crate_id).await.unwrap();

        CrateDetails::new(&mut *conn, &name, &version, req_version, releases)
            .await
            .unwrap()
            .unwrap()
    }

    async fn assert_last_successful_build_equals(
        pool: &Pool,
        package: &str,
        version: &str,
        expected_last_successful_build: Option<Version>,
    ) -> Result<(), Error> {
        let version = version.parse::<Version>()?;
        let mut conn = pool.get_async().await?;
        let details = crate_details(&mut conn, package, version, None).await;

        anyhow::ensure!(
            details.last_successful_build == expected_last_successful_build,
            "didn't expect {:?}",
            details.last_successful_build,
        );

        Ok(())
    }

    #[test]
    fn test_crate_details_documentation_url_is_none_when_url_is_docs_rs() {
        async_wrapper(|env| async move {
            let mut conn = env.async_conn().await?;

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
            let db = env.pool()?;

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
            assert_last_successful_build_equals(db, "foo", "0.0.3", Some("0.0.2".parse().unwrap()))
                .await?;
            assert_last_successful_build_equals(db, "foo", "0.0.4", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.5", Some("0.0.2".parse().unwrap()))
                .await?;
            Ok(())
        });
    }

    #[test]
    fn test_last_successful_build_when_all_releases_failed_or_yanked() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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
            let db = env.pool()?;

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
            assert_last_successful_build_equals(db, "foo", "0.0.2", Some("0.0.4".parse().unwrap()))
                .await?;
            assert_last_successful_build_equals(db, "foo", "0.0.3", None).await?;
            assert_last_successful_build_equals(db, "foo", "0.0.4", None).await?;
            Ok(())
        });
    }

    #[test]
    fn test_releases_should_be_sorted() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            let mut details = crate_details(&mut conn, "foo", "0.2.0", None).await;
            for detail in &mut details.releases {
                detail.release_time = None;
            }

            assert_eq!(
                details.releases,
                vec![
                    Release {
                        version: Version::parse("1.0.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[0].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.12.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[1].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.3.0")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(false),
                        id: details.releases[2].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.2.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(true),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[3].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.2.0-alpha")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[4].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.1.1")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[5].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.1.0")?,
                        build_status: BuildStatus::Success,
                        yanked: Some(false),
                        is_library: Some(true),
                        rustdoc_status: Some(true),
                        id: details.releases[6].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
                    },
                    Release {
                        version: Version::parse("0.0.1")?,
                        build_status: BuildStatus::Failure,
                        yanked: Some(false),
                        is_library: Some(false),
                        rustdoc_status: Some(false),
                        id: details.releases[7].id,
                        target_name: Some("foo".to_owned()),
                        release_time: None,
                        default_target: Some("x86_64-unknown-linux-gnu".into()),
                        doc_targets: Some(vec!["x86_64-unknown-linux-gnu".into()]),
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

            let krate: KrateName = "foo".parse().unwrap();
            let response = env
                .web_app()
                .await
                .get(&format!("/crate/{krate}/0.0.1"))
                .await?;
            response.assert_cache_control(
                CachePolicy::ForeverInCdnAndStaleInBrowser(krate.into()),
                env.config(),
            );

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
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            for version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", *version, None).await;
                assert_eq!(
                    details.latest_release().unwrap().version,
                    Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_prerelease() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            for &version in &["0.0.1", "0.0.2", "0.0.3-pre.1"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
                assert_eq!(
                    details.latest_release().unwrap().version,
                    Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_ignores_yanked() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            for &version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
                assert_eq!(
                    details.latest_release().unwrap().version,
                    Version::parse("0.0.2")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_only_yanked() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            for &version in &["0.0.1", "0.0.2", "0.0.3"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
                assert_eq!(
                    details.latest_release().unwrap().version,
                    Version::parse("0.0.3")?
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_latest_version_in_progress() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
            for &version in &["0.0.1", "0.0.2"] {
                let details = crate_details(&mut conn, "foo", version, None).await;
                assert_eq!(
                    details.latest_release().unwrap().version,
                    Version::parse("0.0.1")?
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
            let db = env.pool()?;

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

            let mut conn = db.get_async().await?;
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

            let mut conn = db.get_async().await?;
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

            let mut conn = db.get_async().await?;
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
                .features(BTreeMap::new())
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
                .collect::<BTreeMap<String, Vec<String>>>();
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
                .collect::<BTreeMap<String, Vec<String>>>();
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
            .collect::<BTreeMap<String, Vec<String>>>();
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

            let mut conn = env.async_conn().await?;
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
            let mut conn = env.async_conn().await?;
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
            let mut conn = env.async_conn().await?;
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

            dbg!(&platform_links);

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
            let response = env.web_app().await.get(dbg!(url)).await.unwrap();
            let status = response.status();
            assert!(
                status.is_success(),
                "no success, status: {}, url: {}, target: {}",
                status,
                url,
                response.redirect_target().unwrap_or_default(),
            );
            let text = response.text().await.unwrap();
            let list1 = dbg!(check_links(
                text.clone(),
                false,
                dbg!(should_contain_redirect)
            ));

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
            let response = env
                .web_app()
                .await
                .get(&dbg!(platform_menu_url))
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{}",
                response.text().await.unwrap()
            );
            response.assert_cache_control(
                CachePolicy::ForeverInCdn(KrateName::from_str("dummy").unwrap().into()),
                env.config(),
            );
            let list2 = dbg!(check_links(
                response.text().await.unwrap(),
                true,
                should_contain_redirect,
            ));
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
            run_check_links_redir(&env, "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/", true).await;
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
                .default_target("aarch64-apple-darwin")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("dummy-ba")
                .version("0.5.0")
                .rustdoc_file("dummy-ba/index.html")
                .rustdoc_file("x86_64-unknown-linux-gnu/dummy-ba/index.html")
                .add_target("x86_64-unknown-linux-gnu")
                .default_target("aarch64-apple-darwin")
                .create()
                .await?;

            check_links(
                // https://github.com/rust-lang/docs.rs/issues/2922
                &env,
                "/crate/dummy-ba/0.5.0/menus/releases/x86_64-unknown-linux-gnu/src/dummy_ba/de.rs.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/x86_64-unknown-linux-gnu/src/dummy_ba/de.rs.html".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/x86_64-unknown-linux-gnu/src/dummy_ba/de.rs.html".to_string(),
                ],
            )
            .await;

            check_links(
                &env,
                "/crate/dummy-ba/latest/menus/releases/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/dummy_ba/".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/dummy_ba/".to_string(),
                ],
            )
            .await;

            check_links(
                &env,
                "/crate/dummy-ba/latest/menus/releases/x86_64-unknown-linux-gnu/dummy_ba/index.html",
                vec![
                    "/crate/dummy-ba/0.5.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/".to_string(),
                    "/crate/dummy-ba/0.4.0/target-redirect/x86_64-unknown-linux-gnu/dummy_ba/".to_string(),
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
            resp.assert_cache_control(
                CachePolicy::ForeverInCdn(KrateName::from_str("dummy").unwrap().into()),
                env.config(),
            );
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
                CachePolicy::ForeverInCdn(KrateName::from_str("dummy").unwrap().into()),
                env.config(),
            )
            .await?;

            Ok(())
        });
    }

    #[tokio::test(flavor = "multi_thread")]
    #[test_case("/crate/rayon/^1.11.0", "/crate/rayon/1.11.0")]
    #[test_case("/crate/rayon/%5E1.11.0", "/crate/rayon/1.11.0")]
    #[test_case("/crate/rayon", "/crate/rayon/latest"; "without trailing slash")]
    #[test_case("/crate/rayon/", "/crate/rayon/latest")]
    async fn test_version_redirects(path: &str, expected_target: &str) -> anyhow::Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name("rayon")
            .version("1.11.0")
            .create()
            .await?;
        let web = env.web_app().await;

        web.assert_redirect(path, expected_target).await?;

        Ok(())
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

            let mut conn = env.async_conn().await?;
            let details = crate_details(&mut conn, "dummy", "0.5.0", None).await;
            assert!(matches!(
                details.fetch_readme(env.storage()?).await,
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

            let resp = env.web_app().await.get("/crate/dummy%3E").await?;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

            Ok(())
        })
    }

    #[test_case("/crate/dummy"; "without")]
    #[test_case("/crate/dummy/"; "slash")]
    fn test_unknown_crate_not_found_doesnt_redirect(path: &str) {
        async_wrapper(|env| async move {
            let resp = env.web_app().await.get(path).await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);

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

            let mut conn = env.async_conn().await?;
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

            let mut conn = env.async_conn().await?;

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

            let mut conn = env.async_conn().await?;

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

            let mut conn = env.async_conn().await?;

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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_stats_no_data() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_conn().await?;

        let stats =
            BuildStatistics::fetch_for_release(&mut conn, CrateId(41), ReleaseId(42)).await?;
        assert!(!stats.has_data());
        assert!(stats.avg_build_duration_release.is_none());
        assert!(stats.avg_build_duration_crate.is_none());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_stats_with_build() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let rid = env
            .fake_release()
            .await
            .name(&FOO)
            .version(V1)
            .create()
            .await?;

        let mut conn = env.async_conn().await?;
        let crate_id = sqlx::query_scalar!(
            r#"
        SELECT crate_id as "id: CrateId"
        FROM releases
        WHERE id = $1
        "#,
            rid as _
        )
        .fetch_one(&mut *conn)
        .await?;

        let stats = BuildStatistics::fetch_for_release(&mut conn, crate_id, rid).await?;
        assert!(stats.has_data());
        assert!(stats.avg_build_duration_release.is_some());
        assert!(stats.avg_build_duration_crate.is_some());

        assert!(
            !BuildStatistics::fetch_for_release(&mut conn, CrateId(41), ReleaseId(42))
                .await?
                .has_data()
        );

        Ok(())
    }
}
