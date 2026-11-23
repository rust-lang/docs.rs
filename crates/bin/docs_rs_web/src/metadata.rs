use crate::{match_release::MatchedRelease, utils::get_correct_docsrs_style_file};
use anyhow::{Context as _, Result};
use docs_rs_database::crate_details::parse_doc_targets;
use docs_rs_types::{BuildId, KrateName, ReqVersion, Version};
use serde::Serialize;

/// MetaData used in header
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MetaData {
    pub(crate) name: KrateName,
    /// The exact version of the release being shown.
    pub(crate) version: Version,
    /// The version identifier in the request that was used to request this page.
    /// This might be any of the variants of `ReqVersion`, but
    /// due to a canonicalization step, it is either an Exact version, or `/latest/`
    /// most of the time.
    pub(crate) req_version: ReqVersion,
    pub(crate) description: Option<String>,
    pub(crate) target_name: Option<String>,
    pub(crate) rustdoc_status: Option<bool>,
    pub(crate) default_target: Option<String>,
    pub(crate) doc_targets: Option<Vec<String>>,
    pub(crate) yanked: Option<bool>,
    /// CSS file to use depending on the rustdoc version used to generate this version of this
    /// crate.
    pub(crate) rustdoc_css_file: Option<String>,
    /// The latest successful build.
    pub(crate) latest_build_id: Option<BuildId>,
}

impl MetaData {
    pub(crate) async fn from_crate(
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
        version: &Version,
        req_version: Option<ReqVersion>,
    ) -> Result<MetaData> {
        let row = sqlx::query!(
            r#"SELECT
                crates.name as "name: KrateName",
                releases.version,
                releases.description,
                releases.target_name,
                releases.rustdoc_status,
                releases.default_target,
                releases.doc_targets,
                releases.yanked,
                builds.id as "latest_build_id?: BuildId",
                builds.rustc_version as "rustc_version?"
            FROM releases
            INNER JOIN crates ON crates.id = releases.crate_id
            LEFT JOIN LATERAL (
                SELECT * FROM builds
                WHERE builds.rid = releases.id AND builds.build_status = 'success'
                ORDER BY builds.build_finished
                DESC LIMIT 1
            ) AS builds ON true
            WHERE crates.name = $1 AND releases.version = $2"#,
            name as _,
            version as _,
        )
        .fetch_one(&mut *conn)
        .await
        .context("error fetching crate metadata")?;

        Ok(MetaData {
            name: row.name,
            version: version.clone(),
            req_version: req_version.unwrap_or_else(|| ReqVersion::Exact(version.clone())),
            description: row.description,
            target_name: row.target_name,
            rustdoc_status: row.rustdoc_status,
            default_target: row.default_target,
            doc_targets: row.doc_targets.as_ref().map(parse_doc_targets),
            yanked: row.yanked,
            rustdoc_css_file: row
                .rustc_version
                .as_deref()
                .map(get_correct_docsrs_style_file)
                .transpose()?,
            latest_build_id: row.latest_build_id,
        })
    }

    /// Only fetches the description and the latest successful build,
    /// the rest comes from the matched release.
    pub(crate) async fn from_matched_release(
        conn: &mut sqlx::PgConnection,
        matched: &MatchedRelease,
    ) -> Result<MetaData> {
        let row = sqlx::query!(
            r#"SELECT
                releases.description,
                builds.id as "latest_build_id?: BuildId",
                builds.rustc_version as "rustc_version?"
            FROM releases
            LEFT JOIN LATERAL (
                SELECT id, rustc_version FROM builds
                WHERE builds.rid = releases.id AND builds.build_status = 'success'
                ORDER BY builds.build_finished
                DESC LIMIT 1
            ) AS builds ON true
            WHERE releases.id = $1"#,
            matched.release.id as _,
        )
        .fetch_one(&mut *conn)
        .await
        .context("error fetching crate metadata")?;

        let release = &matched.release;
        Ok(MetaData {
            name: matched
                .corrected_name
                .as_ref()
                .unwrap_or(&matched.name)
                .clone(),
            version: release.version.clone(),
            req_version: matched.req_version.clone(),
            description: row.description,
            target_name: release.target_name.clone(),
            rustdoc_status: release.rustdoc_status,
            default_target: release.default_target.clone(),
            doc_targets: release.doc_targets(),
            yanked: release.yanked,
            rustdoc_css_file: row
                .rustc_version
                .as_deref()
                .map(get_correct_docsrs_style_file)
                .transpose()?,
            latest_build_id: row.latest_build_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::TestEnvironment;

    use super::*;
    use crate::match_release::match_version;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::{
        BuildStatus,
        testing::{FOO, V0_1},
    };
    use serde_json::json;

    #[test]
    fn serialize_metadata() {
        let mut metadata = MetaData {
            name: "serde".parse().unwrap(),
            version: "1.0.0".parse().unwrap(),
            req_version: ReqVersion::Latest,
            description: Some("serde does stuff".to_string()),
            target_name: None,
            rustdoc_status: Some(true),
            default_target: Some("x86_64-unknown-linux-gnu".to_string()),
            doc_targets: Some(vec![
                "x86_64-unknown-linux-gnu".to_string(),
                "arm64-unknown-linux-gnu".to_string(),
            ]),
            yanked: Some(false),
            rustdoc_css_file: Some("rustdoc.css".to_string()),
            latest_build_id: Some(BuildId(42)),
        };

        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "req_version": "latest",
            "description": "serde does stuff",
            "target_name": null,
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
            "latest_build_id": 42,
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());

        metadata.target_name = Some("serde_lib_name".to_string());
        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "req_version": "latest",
            "description": "serde does stuff",
            "target_name": "serde_lib_name",
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
            "latest_build_id": 42,
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());

        metadata.description = None;
        let correct_json = json!({
            "name": "serde",
            "version": "1.0.0",
            "req_version": "latest",
            "description": null,
            "target_name": "serde_lib_name",
            "rustdoc_status": true,
            "default_target": "x86_64-unknown-linux-gnu",
            "doc_targets": [
                "x86_64-unknown-linux-gnu",
                "arm64-unknown-linux-gnu",
            ],
            "yanked": false,
            "rustdoc_css_file": "rustdoc.css",
            "latest_build_id": 42,
        });

        assert_eq!(correct_json, serde_json::to_value(&metadata).unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_from_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;

        env.fake_release()
            .await
            .name("foo")
            .version("0.1.0")
            .create()
            .await?;

        let mut conn = env.async_conn().await?;
        let metadata =
            MetaData::from_crate(&mut conn, &FOO, &V0_1, Some(ReqVersion::Latest)).await?;
        assert!(metadata.latest_build_id.is_some());
        assert_eq!(
            metadata,
            MetaData {
                name: "foo".parse().unwrap(),
                version: "0.1.0".parse().unwrap(),
                req_version: ReqVersion::Latest,
                description: Some("Fake package".to_string()),
                target_name: Some("foo".to_string()),
                rustdoc_status: Some(true),
                default_target: Some("x86_64-unknown-linux-gnu".to_string()),
                doc_targets: Some(vec!["x86_64-unknown-linux-gnu".to_string()]),
                yanked: Some(false),
                rustdoc_css_file: Some("rustdoc.css".to_string()),
                latest_build_id: metadata.latest_build_id,
            },
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_ignores_later_failed_build() -> Result<()> {
        let env = TestEnvironment::new().await?;

        env.fake_release()
            .await
            .name("foo")
            .version("0.1.0")
            .builds(vec![
                FakeBuild::default().rustc_version("rustc 1.50.0-nightly (abc 2021-01-01)"),
                FakeBuild::default()
                    .rustc_version("rustc 1.90.0-nightly (def 2026-01-01)")
                    .build_status(BuildStatus::Failure),
            ])
            .create()
            .await?;

        let mut conn = env.async_conn().await?;
        let from_crate =
            MetaData::from_crate(&mut conn, &FOO, &V0_1, Some(ReqVersion::Latest)).await?;
        let matched = match_version(&mut conn, &FOO, &ReqVersion::Latest).await?;
        let from_matched = MetaData::from_matched_release(&mut conn, &matched).await?;

        // the failed build has a newer rustc, which would pick another CSS file
        assert_eq!(from_crate.rustdoc_css_file.as_deref(), Some("rustdoc.css"));
        assert_eq!(from_matched, from_crate);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_from_matched_release_without_successful_build() -> Result<()> {
        let env = TestEnvironment::new().await?;

        env.fake_release()
            .await
            .name("foo")
            .version("0.1.0")
            .builds(vec![
                FakeBuild::default().build_status(BuildStatus::InProgress),
            ])
            .create()
            .await?;

        let mut conn = env.async_conn().await?;
        let matched = match_version(&mut conn, &FOO, &ReqVersion::Exact(V0_1.clone())).await?;
        let metadata = MetaData::from_matched_release(&mut conn, &matched).await?;
        assert_eq!(metadata.latest_build_id, None);
        assert_eq!(metadata.rustdoc_css_file, None);
        Ok(())
    }
}
