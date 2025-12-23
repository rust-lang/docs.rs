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

pub fn latest_release(releases: &[Release]) -> Option<&Release> {
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
