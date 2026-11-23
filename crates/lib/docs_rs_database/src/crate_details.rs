use crate::Krate;
use anyhow::Result;
use chrono::{DateTime, Utc};
use docs_rs_types::{BuildStatus, CrateId, KrateName, ReleaseId, Version};
use serde::Deserialize as _;
use serde_json::Value;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Release {
    pub id: ReleaseId,
    pub version: Version,
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
    pub default_target: Option<String>,
    pub doc_targets: Option<Value>,
    pub release_time: Option<DateTime<Utc>>,
    pub is_latest: bool,
}

impl Release {
    pub fn doc_targets(&self) -> Option<Vec<String>> {
        self.doc_targets.as_ref().map(parse_doc_targets)
    }
}

pub fn parse_doc_targets(targets: &Value) -> Vec<String> {
    let mut targets = Vec::<String>::deserialize(targets).unwrap_or_default();
    targets.sort_unstable();
    targets
}

/// Return all releases for a crate, sorted in descending order by semver
pub async fn releases_for_crate(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
) -> Result<Vec<Release>, anyhow::Error> {
    let mut releases = sqlx::query_as!(
        Release,
        r#"SELECT
             releases.id as "id: ReleaseId",
             releases.version as "version: Version",
             release_build_status.build_status as "build_status!: BuildStatus",
             releases.yanked,
             releases.is_library,
             releases.rustdoc_status,
             releases.release_time,
             releases.target_name,
             releases.default_target,
             releases.doc_targets,
             COALESCE(releases.id = crates.latest_version_id, false) as "is_latest!"
         FROM releases
         INNER JOIN release_build_status ON releases.id = release_build_status.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE
             releases.crate_id = $1"#,
        crate_id.0,
    )
    .fetch_all(&mut *conn)
    .await?;

    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
}

/// Return the crate, the release for `requested_version` (latest when `None`) and the crate's
/// latest release (`crates.latest_version_id`, see [`latest_release`]), looking the crate up
/// by its normalized name.
///
/// `None` when no row matched: the crate doesn't exist, or it has neither the requested
/// release nor a latest release.
pub async fn release_and_latest(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    requested_version: Option<&Version>,
) -> Result<Option<(Krate, Option<Release>, Option<Release>)>> {
    // same release columns as `releases_for_crate`
    let rows = sqlx::query!(
        r#"SELECT
             crates.id as "crate_id: CrateId",
             crates.name as "crate_name: KrateName",
             releases.id as "id: ReleaseId",
             releases.version as "version: Version",
             release_build_status.build_status as "build_status!: BuildStatus",
             releases.yanked,
             releases.is_library,
             releases.rustdoc_status,
             releases.release_time,
             releases.target_name,
             releases.default_target,
             releases.doc_targets,
             COALESCE(releases.id = crates.latest_version_id, false) as "is_latest!"
         FROM crates
         INNER JOIN releases ON releases.crate_id = crates.id
         INNER JOIN release_build_status ON releases.id = release_build_status.rid
         WHERE
             normalize_crate_name(crates.name) = normalize_crate_name($1) AND
             (releases.version = $2 OR releases.id = crates.latest_version_id)"#,
        name as _,
        requested_version as _,
    )
    .fetch_all(&mut *conn)
    .await?;

    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let krate = Krate {
        id: first.crate_id,
        name: first.crate_name.clone(),
    };

    let releases: Vec<Release> = rows
        .into_iter()
        .map(|row| Release {
            id: row.id,
            version: row.version,
            build_status: row.build_status,
            yanked: row.yanked,
            is_library: row.is_library,
            rustdoc_status: row.rustdoc_status,
            target_name: row.target_name,
            default_target: row.default_target,
            doc_targets: row.doc_targets,
            release_time: row.release_time,
            is_latest: row.is_latest,
        })
        .collect();

    let latest_release = releases.iter().find(|release| release.is_latest).cloned();
    let requested_release = match requested_version {
        Some(version) => releases
            .into_iter()
            .find(|release| &release.version == version),
        None => latest_release.clone(),
    };
    Ok(Some((krate, requested_release, latest_release)))
}

/// Pick the "latest" release worth pointing users at: non-yanked, build is no longer
/// in-progress, preferring stable over prerelease. Returns `None` when no release meets
/// this bar — either every release is yanked, or the only non-yanked releases are still
/// building. Callers should treat `None` as "no canonical latest" (e.g. `/latest/` 404s,
/// `latest_version_id` is NULL, topbar hides the "Go to latest" button).
pub fn latest_release(releases: &[Release]) -> Option<&Release> {
    fn eligible(release: &Release) -> bool {
        let not_yanked = release.yanked.is_none() || release.yanked == Some(false);
        not_yanked && release.build_status != BuildStatus::InProgress
    }

    // releases are sorted by version desc, so `find` returns the highest match.
    // Prefer non-prerelease, then fall back to prerelease if nothing else qualifies.
    releases
        .iter()
        .find(|r| eligible(r) && r.version.pre.is_empty())
        .or_else(|| releases.iter().find(|r| eligible(r)))
}

pub async fn update_latest_version_id(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
) -> Result<()> {
    let releases = releases_for_crate(conn, crate_id).await?;

    sqlx::query!(
        "UPDATE crates
         SET latest_version_id = $2
         WHERE id = $1",
        crate_id.0,
        latest_release(&releases).map(|release| release.id.0),
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        releases::{initialize_crate, initialize_release},
        testing::TestDatabase,
    };
    use docs_rs_types::testing::{BAR, FOO, V1};

    fn release(version: &str, yanked: bool, build_status: BuildStatus) -> Release {
        Release {
            id: ReleaseId(0),
            version: Version::parse(version).unwrap(),
            build_status,
            yanked: Some(yanked),
            is_library: Some(true),
            rustdoc_status: Some(true),
            target_name: None,
            default_target: None,
            doc_targets: None,
            release_time: None,
            is_latest: false,
        }
    }

    /// Helper mirroring how `releases_for_crate` returns the slice: sorted by version desc.
    fn sorted(mut releases: Vec<Release>) -> Vec<Release> {
        releases.sort_by(|a, b| b.version.cmp(&a.version));
        releases
    }

    #[test]
    fn picks_highest_when_all_eligible() {
        let releases = sorted(vec![
            release("0.0.1", false, BuildStatus::Success),
            release("0.0.3", false, BuildStatus::Success),
            release("0.0.2", false, BuildStatus::Success),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.0.3").unwrap()
        );
    }

    #[test]
    fn prefers_stable_over_prerelease() {
        let releases = sorted(vec![
            release("0.0.1", false, BuildStatus::Success),
            release("0.0.3-pre.1", false, BuildStatus::Success),
            release("0.0.2", false, BuildStatus::Success),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.0.2").unwrap()
        );
    }

    #[test]
    fn picks_highest_prerelease_when_no_stable_exists() {
        let releases = sorted(vec![
            release("0.0.3-pre.1", false, BuildStatus::Success),
            release("0.0.2-pre.1", false, BuildStatus::Success),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.0.3-pre.1").unwrap()
        );
    }

    #[test]
    fn skips_yanked_releases() {
        let releases = sorted(vec![
            release("0.0.1", false, BuildStatus::Success),
            release("0.0.3", true, BuildStatus::Success),
            release("0.0.2", false, BuildStatus::Success),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.0.2").unwrap()
        );
    }

    #[test]
    fn returns_none_when_all_releases_are_yanked() {
        let releases = sorted(vec![
            release("0.0.1", true, BuildStatus::Success),
            release("0.0.3", true, BuildStatus::Success),
            release("0.0.2", true, BuildStatus::Success),
        ]);
        assert!(latest_release(&releases).is_none());
    }

    #[test]
    fn prefers_non_in_progress() {
        let releases = sorted(vec![
            release("0.0.1", false, BuildStatus::Success),
            release("0.0.2", false, BuildStatus::InProgress),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.0.1").unwrap()
        );
    }

    #[test]
    fn returns_none_when_only_in_progress_exists() {
        // An in-progress build isn't a canonical "latest" — no docs to show yet.
        let releases = sorted(vec![release("0.0.1", false, BuildStatus::InProgress)]);
        assert!(latest_release(&releases).is_none());
    }

    #[test]
    fn yanked_latest_with_unyanked_prerelease_picks_prerelease() {
        // Regression for the topbar bug: latest stable is yanked, a non-yanked prerelease
        // exists. We must not return the yanked stable.
        let releases = sorted(vec![
            release("0.2.0", true, BuildStatus::Success),
            release("0.2.0-pre.1", false, BuildStatus::Success),
        ]);
        assert_eq!(
            latest_release(&releases).unwrap().version,
            Version::parse("0.2.0-pre.1").unwrap()
        );
    }

    #[test]
    fn empty_releases_returns_none() {
        assert!(latest_release(&[]).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_and_latest_finds_crate_by_normalized_name() -> Result<()> {
        let db = TestDatabase::with_test_config().await?;
        let mut conn = db.async_conn().await?;

        let stored: KrateName = "foo-bar".parse()?;
        let crate_id = initialize_crate(&mut conn, &stored).await?;
        initialize_release(&mut conn, crate_id, &V1).await?;
        // same version in another crate must not match
        let other_id = initialize_crate(&mut conn, &BAR).await?;
        initialize_release(&mut conn, other_id, &V1).await?;

        let (krate, release, latest) =
            release_and_latest(&mut conn, &"foo_bar".parse()?, Some(&V1))
                .await?
                .unwrap();
        assert_eq!(
            krate,
            Krate {
                id: crate_id,
                name: stored
            }
        );
        assert_eq!(release.unwrap().version, V1);
        // no finished build, so there's no latest release
        assert!(latest.is_none());

        assert_eq!(release_and_latest(&mut conn, &FOO, Some(&V1)).await?, None);
        Ok(())
    }
}
