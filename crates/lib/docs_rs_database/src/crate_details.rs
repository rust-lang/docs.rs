use anyhow::Result;
use chrono::{DateTime, Utc};
use docs_rs_types::{BuildStatus, CrateId, ReleaseId, Version};
use futures_util::TryStreamExt as _;
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
    pub doc_targets: Option<Vec<String>>,
    pub release_time: Option<DateTime<Utc>>,
}

pub fn parse_doc_targets(targets: Value) -> Vec<String> {
    let mut targets: Vec<String> = serde_json::from_value(targets).unwrap_or_default();
    targets.sort_unstable();
    targets
}

/// Return all releases for a crate, sorted in descending order by semver
pub async fn releases_for_crate(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
) -> Result<Vec<Release>, anyhow::Error> {
    let mut releases: Vec<Release> = sqlx::query!(
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
             releases.doc_targets
         FROM releases
         INNER JOIN release_build_status ON releases.id = release_build_status.rid
         WHERE
             releases.crate_id = $1"#,
        crate_id.0,
    )
    .fetch(&mut *conn)
    .try_filter_map(|row| async move {
        Ok(Some(Release {
            id: row.id,
            version: row.version,
            build_status: row.build_status,
            yanked: row.yanked,
            is_library: row.is_library,
            rustdoc_status: row.rustdoc_status,
            target_name: row.target_name,
            default_target: row.default_target,
            doc_targets: row.doc_targets.map(parse_doc_targets),
            release_time: row.release_time,
        }))
    })
    .try_collect()
    .await?;

    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
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
}
