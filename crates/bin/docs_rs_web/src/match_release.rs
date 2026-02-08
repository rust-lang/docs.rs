use crate::error::AxumNope;
use anyhow::{Context as _, Result};
use docs_rs_database::crate_details::Release;
use docs_rs_types::{BuildStatus, CrateId, KrateName, ReqVersion, Version, VersionReq};
use tracing::instrument;

#[derive(Debug)]
pub(crate) struct MatchedRelease {
    /// crate name
    pub name: KrateName,

    /// The crate name that was found when attempting to load a crate release.
    /// `match_version` will attempt to match a provided crate name against similar crate names with
    /// dashes (`-`) replaced with underscores (`_`) and vice versa.
    pub corrected_name: Option<KrateName>,

    /// what kind of version did we get in the request? ("latest", semver, exact)
    pub req_version: ReqVersion,

    /// the matched release
    pub release: Release,

    /// all releases since we have them anyways and so we can pass them to CrateDetails
    pub(crate) all_releases: Vec<Release>,
}

impl MatchedRelease {
    pub(crate) fn assume_exact_name(self) -> Result<Self, AxumNope> {
        if self.corrected_name.is_none() {
            Ok(self)
        } else {
            Err(AxumNope::CrateNotFound)
        }
    }

    pub(crate) fn into_exactly_named(self) -> Self {
        if let Some(corrected_name) = self.corrected_name {
            Self {
                name: corrected_name.to_owned(),
                corrected_name: None,
                ..self
            }
        } else {
            self
        }
    }

    pub(crate) fn into_exactly_named_or_else<F>(self, f: F) -> Result<Self, AxumNope>
    where
        F: FnOnce(&KrateName, &ReqVersion) -> AxumNope,
    {
        if let Some(corrected_name) = self.corrected_name {
            Err(f(&corrected_name, &self.req_version))
        } else {
            Ok(self)
        }
    }

    /// Canonicalize the version from the request
    ///
    /// Mainly:
    /// * "newest"/"*" or empty -> "latest" in the URL
    /// * any other semver requirement -> specific version in the URL
    pub(crate) fn into_canonical_req_version(self) -> Self {
        match self.req_version {
            ReqVersion::Exact(_) | ReqVersion::Latest => self,
            ReqVersion::Semver(version_req) => {
                if version_req == VersionReq::STAR {
                    Self {
                        req_version: ReqVersion::Latest,
                        ..self
                    }
                } else {
                    Self {
                        req_version: ReqVersion::Exact(self.release.version.clone()),
                        ..self
                    }
                }
            }
        }
    }

    /// translate this MatchRelease into a specific semver::Version while canonicalizing the
    /// version specification.
    pub(crate) fn into_canonical_req_version_or_else<F>(self, f: F) -> Result<Self, AxumNope>
    where
        F: FnOnce(&KrateName, &ReqVersion) -> AxumNope,
    {
        let original_req_version = self.req_version.clone();
        let canonicalized = self.into_canonical_req_version();

        if canonicalized.req_version == original_req_version {
            Ok(canonicalized)
        } else {
            Err(f(&canonicalized.name, &canonicalized.req_version))
        }
    }

    pub(crate) fn into_version(self) -> Version {
        self.release.version
    }

    pub(crate) fn build_status(&self) -> BuildStatus {
        self.release.build_status
    }

    pub(crate) fn rustdoc_status(&self) -> bool {
        self.release.rustdoc_status.unwrap_or(false)
    }

    pub(crate) fn is_latest_url(&self) -> bool {
        matches!(self.req_version, ReqVersion::Latest)
    }
}

fn semver_match<'a, F: Fn(&Release) -> bool>(
    releases: &'a [Release],
    req: &VersionReq,
    filter: F,
) -> Option<&'a Release> {
    // first try standard semver match using `VersionReq::match`, should handle most cases.
    if let Some(release) = releases
        .iter()
        .filter(|release| filter(release))
        .find(|release| req.matches(&release.version))
    {
        Some(release)
    } else if req == &VersionReq::STAR {
        // semver `*` does not match pre-releases.
        // So when we only have pre-releases, `VersionReq::STAR` would lead to an
        // empty result.
        // In this case we just return the latest prerelease instead of nothing.
        releases.iter().find(|release| filter(release))
    } else {
        None
    }
}

/// Checks the database for crate releases that match the given name and version.
///
/// `version` may be an exact version number or loose semver version requirement. The return value
/// will indicate whether the given version exactly matched a version number from the database.
///
/// This function will also check for crates where dashes in the name (`-`) have been replaced with
/// underscores (`_`) and vice-versa. The return value will indicate whether the crate name has
/// been matched exactly, or if there has been a "correction" in the name that matched instead.
#[instrument(skip(conn))]
pub(crate) async fn match_version(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    input_version: &ReqVersion,
) -> Result<MatchedRelease, AxumNope> {
    let (crate_id, name, corrected_name) = {
        let row = sqlx::query!(
            r#"
             SELECT
                id as "id: CrateId",
                name as "name: KrateName"
             FROM crates
             WHERE normalize_crate_name(name) = normalize_crate_name($1)"#,
            name as _,
        )
        .fetch_optional(&mut *conn)
        .await
        .context("error fetching crate")?
        .ok_or(AxumNope::CrateNotFound)?;

        if row.name != name {
            (row.id, name, Some(row.name))
        } else {
            (row.id, name, None)
        }
    };

    // first load and parse all versions of this crate,
    // `releases_for_crate` is already sorted, newest version first.
    let releases = docs_rs_database::crate_details::releases_for_crate(conn, crate_id)
        .await
        .context("error fetching releases for crate")?;

    if releases.is_empty() {
        return Err(AxumNope::CrateNotFound);
    }

    let req_semver: VersionReq = match input_version {
        ReqVersion::Exact(parsed_req_version) => {
            if let Some(release) = releases
                .iter()
                .find(|release| &release.version == parsed_req_version)
            {
                return Ok(MatchedRelease {
                    name: name.clone(),
                    corrected_name,
                    req_version: input_version.clone(),
                    release: release.clone(),
                    all_releases: releases,
                });
            }

            if let Ok(version_req) = VersionReq::parse(&parsed_req_version.to_string()) {
                // when we don't find a release with exact version,
                // we try to interpret it as a semver requirement.
                // A normal semver version ("1.2.3") is equivalent to a caret semver requirement.
                version_req
            } else {
                return Err(AxumNope::VersionNotFound);
            }
        }
        ReqVersion::Latest => VersionReq::STAR,
        ReqVersion::Semver(version_req) => version_req.clone(),
    };

    // when matching semver requirements,
    // we generally only want to look at non-yanked releases,
    // excluding releases which just contain in-progress builds
    if let Some(release) = semver_match(&releases, &req_semver, |r: &Release| {
        r.build_status != BuildStatus::InProgress && (r.yanked.is_none() || r.yanked == Some(false))
    }) {
        return Ok(MatchedRelease {
            name: name.to_owned(),
            corrected_name,
            req_version: input_version.clone(),
            release: release.clone(),
            all_releases: releases,
        });
    }

    // when we don't find any match with "normal" releases, we also look into in-progress releases
    if let Some(release) = semver_match(&releases, &req_semver, |r: &Release| {
        r.yanked.is_none() || r.yanked == Some(false)
    }) {
        return Ok(MatchedRelease {
            name: name.to_owned(),
            corrected_name,
            req_version: input_version.clone(),
            release: release.clone(),
            all_releases: releases,
        });
    }

    // Since we return with a CrateNotFound earlier if the db reply is empty,
    // we know that versions were returned but none satisfied the version requirement.
    // This can only happen when all versions are yanked.
    Err(AxumNope::VersionNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestEnvironment, async_wrapper};
    use docs_rs_database::Pool;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::{ReleaseId, testing::FOO};
    use std::str::FromStr as _;

    async fn release(version: &str, env: &TestEnvironment) -> ReleaseId {
        let version = Version::parse(version).unwrap();
        env.fake_release()
            .await
            .name("foo")
            .version(version)
            .create()
            .await
            .unwrap()
    }

    async fn version(v: Option<&str>, pool: &Pool) -> Option<Version> {
        let mut conn = pool.get_async().await.unwrap();
        let version = match_version(
            &mut conn,
            &FOO,
            &ReqVersion::from_str(v.unwrap_or_default()).unwrap(),
        )
        .await
        .ok()?
        .assume_exact_name()
        .ok()?
        .into_version();
        Some(version)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn exact(version: &'static str) -> Option<Version> {
        version.parse().ok()
    }

    #[allow(clippy::unnecessary_wraps)]
    fn semver(version: &'static str) -> Option<Version> {
        version.parse().ok()
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/223
    fn prereleases_are_not_considered_for_semver() {
        async_wrapper(|env| async move {
            let db = env.pool()?;
            let version = |v| version(v, db);
            let release = |v| release(v, &env);

            release("0.3.1-pre").await;
            for search in &["*", "newest", "latest"] {
                assert_eq!(version(Some(search)).await, semver("0.3.1-pre"));
            }

            release("0.3.1-alpha").await;
            assert_eq!(version(Some("0.3.1-alpha")).await, exact("0.3.1-alpha"));

            release("0.3.0").await;
            let three = semver("0.3.0");
            assert_eq!(version(None).await, three);
            // same thing but with "*"
            assert_eq!(version(Some("*")).await, three);
            // make sure exact matches still work
            assert_eq!(version(Some("0.3.0")).await, exact("0.3.0"));

            Ok(())
        });
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/1682
    fn prereleases_are_considered_when_others_dont_match() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

            // normal release
            release("1.0.0", &env).await;
            // prereleases
            release("2.0.0-alpha.1", &env).await;
            release("2.0.0-alpha.2", &env).await;

            // STAR gives me the prod release
            assert_eq!(version(Some("*"), db).await, exact("1.0.0"));

            // prerelease query gives me the latest prerelease
            assert_eq!(
                version(Some(">=2.0.0-alpha"), db).await,
                exact("2.0.0-alpha.2")
            );

            Ok(())
        })
    }

    #[test]
    // vaguely related to https://github.com/rust-lang/docs.rs/issues/395
    fn metadata_has_no_effect() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

            release("0.1.0+4.1", &env).await;
            release("0.1.1", &env).await;
            assert_eq!(version(None, db).await, semver("0.1.1"));
            release("0.5.1+zstd.1.4.4", &env).await;
            assert_eq!(version(None, db).await, semver("0.5.1+zstd.1.4.4"));
            assert_eq!(version(Some("0.5"), db).await, semver("0.5.1+zstd.1.4.4"));
            assert_eq!(
                version(Some("0.5.1+zstd.1.4.4"), db).await,
                exact("0.5.1+zstd.1.4.4")
            );

            Ok(())
        });
    }

    #[test]
    fn in_progress_releases_are_ignored_when_others_match() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

            // normal release
            release("1.0.0", &env).await;

            // in progress release
            env.fake_release()
                .await
                .name("foo")
                .version("1.1.0")
                .builds(vec![
                    FakeBuild::default().build_status(BuildStatus::InProgress),
                ])
                .create()
                .await?;

            // STAR gives me the prod release
            assert_eq!(version(Some("*"), db).await, exact("1.0.0"));

            // exact-match query gives me the in progress release
            assert_eq!(version(Some("=1.1.0"), db).await, exact("1.1.0"));

            Ok(())
        })
    }

    #[test]
    // https://github.com/rust-lang/docs.rs/issues/221
    fn yanked_crates_are_not_considered() {
        async_wrapper(|env| async move {
            let db = env.pool()?;

            let release_id = release("0.3.0", &env).await;

            sqlx::query!(
                "UPDATE releases SET yanked = true WHERE id = $1 AND version = '0.3.0'",
                release_id.0
            )
            .execute(&mut *db.get_async().await?)
            .await?;

            assert_eq!(version(None, db).await, None);
            assert_eq!(version(Some("0.3"), db).await, None);

            release("0.1.0+4.1", &env).await;
            assert_eq!(version(Some("0.1.0+4.1"), db).await, exact("0.1.0+4.1"));
            assert_eq!(version(None, db).await, semver("0.1.0+4.1"));

            Ok(())
        });
    }
}
