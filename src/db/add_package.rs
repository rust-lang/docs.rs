use crate::{
    db::types::{BuildStatus, Feature},
    docbuilder::DocCoverage,
    error::Result,
    registry_api::{CrateData, CrateOwner, ReleaseData},
    storage::CompressionAlgorithm,
    utils::{rustc_version::parse_rustc_date, MetadataPackage},
    web::crate_details::{latest_release, releases_for_crate},
};
use anyhow::{anyhow, Context};
use derive_more::Display;
use futures_util::stream::TryStreamExt;
use serde::Serialize;
use serde_json::Value;
use slug::slugify;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::Path,
};
use tracing::{debug, error, info, instrument};

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct CrateId(pub i32);

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct ReleaseId(pub i32);

#[derive(Debug, Clone, Copy, Display, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct BuildId(pub i32);

/// Adds a package into database.
///
/// Package must be built first.
///
/// NOTE: `source_files` refers to the files originally in the crate,
/// not the files generated by rustdoc.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(conn, compression_algorithms))]
pub(crate) async fn finish_release(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
    release_id: ReleaseId,
    metadata_pkg: &MetadataPackage,
    source_dir: &Path,
    default_target: &str,
    source_files: Value,
    doc_targets: Vec<String>,
    registry_data: &ReleaseData,
    has_docs: bool,
    has_examples: bool,
    compression_algorithms: impl IntoIterator<Item = CompressionAlgorithm>,
    repository_id: Option<i32>,
    archive_storage: bool,
    source_size: u64,
) -> Result<()> {
    debug!("updating release data");
    let dependencies = convert_dependencies(metadata_pkg);
    let rustdoc = get_rustdoc(metadata_pkg, source_dir).unwrap_or(None);
    let readme = get_readme(metadata_pkg, source_dir).unwrap_or(None);
    let features = get_features(metadata_pkg);
    let is_library = metadata_pkg.is_library();

    let result = sqlx::query!(
        r#"UPDATE releases
           SET release_time = $2,
               dependencies = $3,
               target_name = $4,
               yanked = $5,
               rustdoc_status = $6,
               test_status = $7,
               license = $8,
               repository_url = $9,
               homepage_url = $10,
               description = $11,
               description_long = $12,
               readme = $13,
               keywords = $14,
               have_examples = $15,
               downloads = $16,
               files = $17,
               doc_targets = $18,
               is_library = $19,
               documentation_url = $20,
               default_target = $21,
               features = $22,
               repository_id = $23,
               archive_storage = $24,
               source_size = $25
           WHERE id = $1"#,
        release_id.0,
        registry_data.release_time,
        serde_json::to_value(dependencies)?,
        metadata_pkg.package_name(),
        registry_data.yanked,
        has_docs,
        false, // TODO: Add test status somehow
        metadata_pkg.license,
        metadata_pkg.repository,
        metadata_pkg.homepage,
        metadata_pkg.description,
        rustdoc,
        readme,
        serde_json::to_value(&metadata_pkg.keywords)?,
        has_examples,
        registry_data.downloads,
        source_files,
        serde_json::to_value(doc_targets)?,
        is_library,
        metadata_pkg.documentation,
        default_target,
        features as Vec<Feature>,
        repository_id,
        archive_storage,
        source_size as i64,
    )
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() < 1 {
        return Err(anyhow!("Failed to update release"));
    }

    add_keywords_into_database(conn, metadata_pkg, release_id).await?;
    add_compression_into_database(conn, compression_algorithms.into_iter(), release_id).await?;

    update_latest_version_id(&mut *conn, crate_id)
        .await
        .context("couldn't update latest version id")?;

    update_build_status(conn, release_id).await?;

    Ok(())
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

pub async fn update_build_status(
    conn: &mut sqlx::PgConnection,
    release_id: ReleaseId,
) -> Result<()> {
    sqlx::query!(
        "INSERT INTO release_build_status(rid, last_build_time, build_status)
         SELECT
         summary.id,
         summary.last_build_time,
         CASE
           WHEN summary.success_count > 0 THEN 'success'::build_status
           WHEN summary.failure_count > 0 THEN 'failure'::build_status
           ELSE 'in_progress'::build_status
         END as build_status

         FROM (
             SELECT
               r.id,
               MAX(b.build_finished) as last_build_time,
               SUM(CASE WHEN b.build_status = 'success' THEN 1 ELSE 0 END) as success_count,
               SUM(CASE WHEN b.build_status = 'failure' THEN 1 ELSE 0 END) as failure_count
             FROM
               releases as r
               LEFT OUTER JOIN builds AS b on b.rid = r.id
             WHERE
               r.id = $1
             GROUP BY r.id
         ) as summary

         ON CONFLICT (rid) DO UPDATE
         SET
             last_build_time = EXCLUDED.last_build_time,
             build_status=EXCLUDED.build_status",
        release_id.0,
    )
    .execute(&mut *conn)
    .await?;

    let crate_id = crate_id_from_release_id(&mut *conn, release_id).await?;
    update_latest_version_id(&mut *conn, crate_id)
        .await
        .context("couldn't update latest version id")?;

    Ok(())
}

async fn crate_id_from_release_id(
    conn: &mut sqlx::PgConnection,
    release_id: ReleaseId,
) -> Result<CrateId> {
    Ok(sqlx::query_scalar!(
        r#"
        SELECT crate_id as "crate_id: CrateId"
        FROM releases
        WHERE id = $1"#,
        release_id.0,
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[instrument(skip(conn))]
pub(crate) async fn add_doc_coverage(
    conn: &mut sqlx::PgConnection,
    release_id: ReleaseId,
    doc_coverage: DocCoverage,
) -> Result<i32> {
    debug!("Adding doc coverage into database");
    Ok(sqlx::query_scalar!(
        "INSERT INTO doc_coverage (
            release_id, total_items, documented_items,
            total_items_needing_examples, items_with_examples
        )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (release_id) DO UPDATE
                SET
                    total_items = $2,
                    documented_items = $3,
                    total_items_needing_examples = $4,
                    items_with_examples = $5
            RETURNING release_id",
        release_id.0,
        &doc_coverage.total_items,
        &doc_coverage.documented_items,
        &doc_coverage.total_items_needing_examples,
        &doc_coverage.items_with_examples,
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Adds a build into database
#[instrument(skip(conn))]
pub(crate) async fn finish_build(
    conn: &mut sqlx::PgConnection,
    build_id: BuildId,
    rustc_version: &str,
    docsrs_version: &str,
    build_status: BuildStatus,
    documentation_size: Option<u64>,
    errors: Option<&str>,
) -> Result<()> {
    debug!("updating build after finishing");
    let hostname = hostname::get()?;

    let rustc_date = match parse_rustc_date(rustc_version) {
        Ok(date) => Some(date),
        Err(err) => {
            // in the database we see cases where the rustc version is missing
            // in the builds-table. In this case & if we can't parse the version
            // we just want to log an error, but still finish the build.
            error!(
                "Failed to parse date from rustc version \"{}\": {:?}",
                rustc_version, err
            );
            None
        }
    };

    let release_id = sqlx::query_scalar!(
        r#"UPDATE builds
         SET
             rustc_version = $1,
             docsrs_version = $2,
             build_status = $3,
             build_server = $4,
             errors = $5,
             documentation_size = $6,
             rustc_nightly_date = $7,
             build_finished = NOW()
         WHERE
            id = $8
         RETURNING rid as "rid: ReleaseId" "#,
        rustc_version,
        docsrs_version,
        build_status as BuildStatus,
        hostname.to_str().unwrap_or(""),
        errors,
        documentation_size.map(|v| v as i64),
        rustc_date,
        build_id.0,
    )
    .fetch_one(&mut *conn)
    .await?;

    update_build_status(conn, release_id).await?;

    Ok(())
}

#[instrument(skip(conn))]
pub(crate) async fn update_build_with_error(
    conn: &mut sqlx::PgConnection,
    build_id: BuildId,
    errors: Option<&str>,
) -> Result<BuildId> {
    debug!("updating build with error");
    let release_id = sqlx::query_scalar!(
        r#"UPDATE builds
         SET
             build_status = $1,
             errors = $2
         WHERE id = $3
         RETURNING rid as "rid: ReleaseId" "#,
        BuildStatus::Failure as BuildStatus,
        errors,
        build_id.0,
    )
    .fetch_one(&mut *conn)
    .await?;

    update_build_status(conn, release_id).await?;

    Ok(build_id)
}

pub(crate) async fn initialize_crate(conn: &mut sqlx::PgConnection, name: &str) -> Result<CrateId> {
    sqlx::query_scalar!(
        "INSERT INTO crates (name)
         VALUES ($1)
         ON CONFLICT (name) DO UPDATE
         SET -- this `SET` is needed so the id is always returned.
            name = EXCLUDED.name
         RETURNING id",
        name
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(Into::into)
    .map(CrateId)
}

pub(crate) async fn initialize_release(
    conn: &mut sqlx::PgConnection,
    crate_id: CrateId,
    version: &str,
) -> Result<ReleaseId> {
    let release_id = sqlx::query_scalar!(
        r#"INSERT INTO releases (crate_id, version, archive_storage)
         VALUES ($1, $2, TRUE)
         ON CONFLICT (crate_id, version) DO UPDATE
         SET -- this `SET` is needed so the id is always returned.
            version = EXCLUDED.version
         RETURNING id as "id: ReleaseId" "#,
        crate_id.0,
        version
    )
    .fetch_one(&mut *conn)
    .await?;

    update_build_status(conn, release_id).await?;

    Ok(release_id)
}

pub(crate) async fn initialize_build(
    conn: &mut sqlx::PgConnection,
    release_id: ReleaseId,
) -> Result<BuildId> {
    let hostname = hostname::get()?;

    let build_id = sqlx::query_scalar!(
        r#"INSERT INTO builds(rid, build_status, build_server, build_started)
         VALUES ($1, $2, $3, NOW())
         RETURNING id as "id: BuildId" "#,
        release_id.0,
        BuildStatus::InProgress as BuildStatus,
        hostname.to_str().unwrap_or(""),
    )
    .fetch_one(&mut *conn)
    .await?;

    update_build_status(conn, release_id).await?;

    Ok(build_id)
}

/// Convert dependencies into Vec<(String, String, String, bool)>
fn convert_dependencies(pkg: &MetadataPackage) -> Vec<(String, String, String, bool)> {
    pkg.dependencies
        .iter()
        .map(|dependency| {
            let name = dependency.name.clone();
            let version = dependency.req.clone();
            let kind = dependency
                .kind
                .clone()
                .unwrap_or_else(|| "normal".to_string());
            (name, version, kind, dependency.optional)
        })
        .collect()
}

/// Reads features and converts them to Vec<Feature> with default being first
fn get_features(pkg: &MetadataPackage) -> Vec<Feature> {
    let mut features = Vec::with_capacity(pkg.features.len());
    if let Some(subfeatures) = pkg.features.get("default") {
        features.push(Feature::new("default".into(), subfeatures.clone()));
    };
    features.extend(
        pkg.features
            .iter()
            .filter(|(name, _)| *name != "default")
            .map(|(name, subfeatures)| Feature::new(name.clone(), subfeatures.clone())),
    );
    features
}

/// Reads readme if there is any read defined in Cargo.toml of a Package
fn get_readme(pkg: &MetadataPackage, source_dir: &Path) -> Result<Option<String>> {
    let readme_path = source_dir.join(pkg.readme.as_deref().unwrap_or("README.md"));

    if !readme_path.exists() {
        return Ok(None);
    }

    let readme = fs::read_to_string(readme_path)?;

    if readme.is_empty() {
        Ok(None)
    } else if readme.len() > 51200 {
        Ok(Some(format!(
            "(Readme ignored due to being too long. ({} > 51200))",
            readme.len()
        )))
    } else {
        Ok(Some(readme))
    }
}

fn get_rustdoc(pkg: &MetadataPackage, source_dir: &Path) -> Result<Option<String>> {
    if let Some(src_path) = &pkg.targets.first().and_then(|t| t.src_path.as_ref()) {
        let src_path = Path::new(src_path);
        if src_path.is_absolute() {
            read_rust_doc(src_path)
        } else {
            read_rust_doc(&source_dir.join(src_path))
        }
    } else {
        // FIXME: should we care about metabuild targets?
        Ok(None)
    }
}

/// Reads rustdoc from library
fn read_rust_doc(file_path: &Path) -> Result<Option<String>> {
    let reader = fs::File::open(file_path).map(BufReader::new)?;
    let mut rustdoc = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.starts_with("//!") {
            // some lines may or may not have a space between the `//!` and the start of the text
            let line = line.trim_start_matches("//!").trim_start();
            if !line.is_empty() {
                rustdoc.push_str(line);
            }
            rustdoc.push('\n');
        }
    }

    if rustdoc.is_empty() {
        Ok(None)
    } else if rustdoc.len() > 51200 {
        Ok(Some(format!(
            "(Library doc comment ignored due to being too long. ({} > 51200))",
            rustdoc.len()
        )))
    } else {
        Ok(Some(rustdoc))
    }
}

/// Adds keywords into database
async fn add_keywords_into_database(
    conn: &mut sqlx::PgConnection,
    pkg: &MetadataPackage,
    release_id: ReleaseId,
) -> Result<()> {
    let wanted_keywords: HashMap<String, String> = pkg
        .keywords
        .iter()
        .map(|kw| (slugify(kw), kw.clone()))
        .collect();

    let existing_keyword_slugs: HashSet<String> = sqlx::query!(
        "SELECT slug FROM keywords WHERE slug = ANY($1)",
        &wanted_keywords.keys().cloned().collect::<Vec<_>>()[..],
    )
    .fetch(&mut *conn)
    .map_ok(|row| row.slug)
    .try_collect()
    .await?;

    // we create new keywords one-by-one, since most of the time we already have them,
    // and because support for multi-record inserts is a mess without adding a new
    // library
    for (slug, name) in wanted_keywords
        .iter()
        .filter(|(k, _)| !(existing_keyword_slugs.contains(*k)))
    {
        sqlx::query!(
            "INSERT INTO keywords (name, slug) VALUES ($1, $2)",
            name,
            slug
        )
        .execute(&mut *conn)
        .await?;
    }

    sqlx::query!(
        "INSERT INTO keyword_rels (rid, kid)
        SELECT $1 as rid, id as kid
        FROM keywords
        WHERE slug = ANY($2)
        ON CONFLICT DO NOTHING;",
        release_id.0,
        &wanted_keywords.keys().cloned().collect::<Vec<_>>()[..],
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

#[instrument(skip(conn))]
pub async fn update_crate_data_in_database(
    conn: &mut sqlx::PgConnection,
    name: &str,
    registry_data: &CrateData,
) -> Result<()> {
    info!("Updating crate data for {}", name);
    let crate_id = sqlx::query_scalar!(
        r#"SELECT id as "id: CrateId" FROM crates WHERE crates.name = $1"#,
        name
    )
    .fetch_one(&mut *conn)
    .await?;

    update_owners_in_database(conn, &registry_data.owners, crate_id).await?;

    Ok(())
}

/// Adds owners into database
async fn update_owners_in_database(
    conn: &mut sqlx::PgConnection,
    owners: &[CrateOwner],
    crate_id: CrateId,
) -> Result<()> {
    // Update any existing owner data since it is mutable and could have changed since last
    // time we pulled it

    let mut oids: Vec<i32> = Vec::new();

    for owner in owners {
        oids.push(
            sqlx::query_scalar!(
                "INSERT INTO owners (login, avatar, kind)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (login) DO UPDATE
                     SET
                         avatar = EXCLUDED.avatar,
                         kind = EXCLUDED.kind
                 RETURNING id",
                owner.login,
                owner.avatar,
                owner.kind as _,
            )
            .fetch_one(&mut *conn)
            .await?,
        );
    }

    sqlx::query!(
        "INSERT INTO owner_rels (cid, oid)
             SELECT $1,oid
             FROM UNNEST($2::int[]) as oid
             ON CONFLICT (cid,oid)
             DO NOTHING",
        crate_id.0,
        &oids[..]
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "DELETE FROM owner_rels
         WHERE
            cid = $1 AND
            NOT (oid = ANY($2))",
        crate_id.0,
        &oids[..],
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Add the compression algorithms used for this crate to the database
async fn add_compression_into_database<I>(
    conn: &mut sqlx::PgConnection,
    algorithms: I,
    release_id: ReleaseId,
) -> Result<()>
where
    I: Iterator<Item = CompressionAlgorithm>,
{
    for alg in algorithms {
        sqlx::query!(
            "INSERT INTO compression_rels (release, algorithm)
             VALUES ($1, $2)
             ON CONFLICT DO NOTHING;",
            release_id.0,
            &(alg as i32)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::registry_api::OwnerKind;
    use crate::test::*;
    use crate::utils::CargoMetadata;
    use chrono::NaiveDate;
    use test_case::test_case;

    #[test]
    fn test_set_build_to_error() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;
            let release_id = initialize_release(&mut conn, crate_id, "0.1.0").await?;
            let build_id = initialize_build(&mut conn, release_id).await?;

            update_build_with_error(&mut conn, build_id, Some("error message")).await?;

            let row = sqlx::query!(
                r#"SELECT
                rustc_version,
                docsrs_version,
                build_started,
                build_status as "build_status: BuildStatus",
                errors
                FROM builds
                WHERE id = $1"#,
                build_id.0
            )
            .fetch_one(&mut *conn)
            .await?;

            assert!(row.rustc_version.is_none());
            assert!(row.docsrs_version.is_none());
            assert!(row.build_started.is_some());
            assert_eq!(row.build_status, BuildStatus::Failure);
            assert_eq!(row.errors, Some("error message".into()));

            Ok(())
        })
    }

    #[test]
    fn test_finish_build_success_valid_rustc_date() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;
            let release_id = initialize_release(&mut conn, crate_id, "0.1.0").await?;
            let build_id = initialize_build(&mut conn, release_id).await?;

            finish_build(
                &mut conn,
                build_id,
                "rustc 1.84.0-nightly (e7c0d2750 2024-10-15)",
                "docsrs_version",
                BuildStatus::Success,
                None,
                None,
            )
            .await?;

            let row = sqlx::query!(
                r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                errors,
                rustc_nightly_date
                FROM builds
                WHERE id = $1"#,
                build_id.0
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(
                row.rustc_version,
                Some("rustc 1.84.0-nightly (e7c0d2750 2024-10-15)".into())
            );
            assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
            assert_eq!(row.build_status, BuildStatus::Success);
            assert_eq!(
                row.rustc_nightly_date,
                Some(NaiveDate::from_ymd_opt(2024, 10, 15).unwrap())
            );
            assert!(row.errors.is_none());

            Ok(())
        })
    }

    #[test]
    fn test_finish_build_success_invalid_rustc_date() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;
            let release_id = initialize_release(&mut conn, crate_id, "0.1.0").await?;
            let build_id = initialize_build(&mut conn, release_id).await?;

            finish_build(
                &mut conn,
                build_id,
                "rustc_version",
                "docsrs_version",
                BuildStatus::Success,
                Some(42),
                None,
            )
            .await?;

            let row = sqlx::query!(
                r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                documentation_size,
                errors,
                rustc_nightly_date
                FROM builds
                WHERE id = $1"#,
                build_id.0
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(row.rustc_version, Some("rustc_version".into()));
            assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
            assert_eq!(row.build_status, BuildStatus::Success);
            assert_eq!(row.documentation_size, Some(42));
            assert!(row.rustc_nightly_date.is_none());
            assert!(row.errors.is_none());

            Ok(())
        })
    }

    #[test]
    fn test_finish_build_error() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;
            let release_id = initialize_release(&mut conn, crate_id, "0.1.0").await?;
            let build_id = initialize_build(&mut conn, release_id).await?;

            finish_build(
                &mut conn,
                build_id,
                "rustc_version",
                "docsrs_version",
                BuildStatus::Failure,
                None,
                Some("error message"),
            )
            .await?;

            let row = sqlx::query!(
                r#"SELECT
                rustc_version,
                docsrs_version,
                build_status as "build_status: BuildStatus",
                documentation_size,
                errors
                FROM builds
                WHERE id = $1"#,
                build_id.0
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(row.rustc_version, Some("rustc_version".into()));
            assert_eq!(row.docsrs_version, Some("docsrs_version".into()));
            assert_eq!(row.build_status, BuildStatus::Failure);
            assert_eq!(row.errors, Some("error message".into()));
            assert!(row.documentation_size.is_none());

            Ok(())
        })
    }

    #[test]
    fn new_keywords() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;

            let release_id = env
                .async_fake_release()
                .await
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 1".into(), "kw 2".into()])
                .create_async()
                .await?;

            let kw_r = sqlx::query!(
                r#"SELECT
                        kw.name as "name!",
                        kw.slug as "slug!"
                   FROM keywords as kw
                   INNER JOIN keyword_rels as kwr on kw.id = kwr.kid
                   WHERE kwr.rid = $1
                   ORDER BY kw.name,kw.slug"#,
                release_id.0
            )
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|row| (row.name, row.slug))
            .collect::<Vec<_>>();

            assert_eq!(kw_r[0], ("kw 1".into(), "kw-1".into()));
            assert_eq!(kw_r[1], ("kw 2".into(), "kw-2".into()));

            let all_kw = sqlx::query!("SELECT slug FROM keywords ORDER BY slug")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|row| row.slug)
                .collect::<Vec<_>>();

            assert_eq!(all_kw, vec![String::from("kw-1"), "kw-2".into()]);

            Ok(())
        })
    }

    #[test]
    fn keyword_conflict_when_rebuilding_release() {
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 3".into(), "kw 4".into()])
                .create()?;

            // same version so we have the same release
            env.fake_release()
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 3".into(), "kw 4".into()])
                .create()?;

            Ok(())
        })
    }

    #[test]
    fn updated_keywords() {
        async_wrapper(|env| async move {
            env.async_fake_release()
                .await
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 3".into(), "kw 4".into()])
                .create_async()
                .await?;

            let release_id = env
                .async_fake_release()
                .await
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 1".into(), "kw 2".into()])
                .create_async()
                .await?;

            let mut conn = env.async_db().await.async_conn().await;
            let kw_r = sqlx::query!(
                r#"SELECT
                    kw.name as "name!",
                    kw.slug as "slug!"
                 FROM keywords as kw
                 INNER JOIN keyword_rels as kwr on kw.id = kwr.kid
                 WHERE kwr.rid = $1
                 ORDER BY kw.name,kw.slug"#,
                release_id.0
            )
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|row| (row.name, row.slug))
            .collect::<Vec<_>>();

            assert_eq!(kw_r[0], ("kw 1".into(), "kw-1".into()));
            assert_eq!(kw_r[1], ("kw 2".into(), "kw-2".into()));

            let all_kw = sqlx::query!("SELECT slug FROM keywords ORDER BY slug")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|row| row.slug)
                .collect::<Vec<_>>();

            assert_eq!(
                all_kw,
                vec![
                    String::from("kw-1"),
                    "kw-2".into(),
                    "kw-3".into(),
                    "kw-4".into(),
                ]
            );

            Ok(())
        })
    }

    #[test]
    fn new_owner_long_avatar() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;

            let owner1 = CrateOwner {
                avatar: "avatar".repeat(100),
                login: "login".into(),
                kind: OwnerKind::User,
            };

            update_owners_in_database(&mut conn, &[owner1.clone()], crate_id).await?;

            let owner_def = sqlx::query!(
                r#"SELECT login, avatar, kind as "kind: OwnerKind"
                FROM owners"#
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_def.login, owner1.login);
            assert_eq!(owner_def.avatar, owner1.avatar);
            assert_eq!(owner_def.kind, owner1.kind);

            let owner_rel = sqlx::query!(
                "SELECT o.login
                FROM owners o, owner_rels r
                WHERE
                    o.id = r.oid AND
                    r.cid = $1",
                crate_id.0
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_rel.login, owner1.login);

            Ok(())
        })
    }

    #[test]
    fn new_owners() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;

            let owner1 = CrateOwner {
                avatar: "avatar".into(),
                login: "login".into(),
                kind: OwnerKind::User,
            };

            update_owners_in_database(&mut conn, &[owner1.clone()], crate_id).await?;

            let owner_def = sqlx::query!(
                r#"SELECT login, avatar, kind as "kind: OwnerKind"
                FROM owners"#
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_def.login, owner1.login);
            assert_eq!(owner_def.avatar, owner1.avatar);
            assert_eq!(owner_def.kind, owner1.kind);

            let owner_rel = sqlx::query!(
                "SELECT o.login
                FROM owners o, owner_rels r
                WHERE
                    o.id = r.oid AND
                    r.cid = $1",
                crate_id.0
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_rel.login, owner1.login);

            Ok(())
        })
    }

    #[test]
    fn update_owner_details() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;

            // set initial owner details
            update_owners_in_database(
                &mut conn,
                &[CrateOwner {
                    login: "login".into(),
                    avatar: "avatar".into(),
                    kind: OwnerKind::User,
                }],
                crate_id,
            )
            .await?;

            let updated_owner = CrateOwner {
                login: "login".into(),
                avatar: "avatar2".into(),
                kind: OwnerKind::Team,
            };
            update_owners_in_database(&mut conn, &[updated_owner.clone()], crate_id).await?;

            let owner_def =
                sqlx::query!(r#"SELECT login, avatar, kind as "kind: OwnerKind" FROM owners"#)
                    .fetch_one(&mut *conn)
                    .await?;
            assert_eq!(owner_def.login, updated_owner.login);
            assert_eq!(owner_def.avatar, updated_owner.avatar);
            assert_eq!(owner_def.kind, updated_owner.kind);

            let owner_rel = sqlx::query!(
                "SELECT o.login
                FROM owners o, owner_rels r
                WHERE
                    o.id = r.oid AND
                    r.cid = $1",
                crate_id.0
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_rel.login, updated_owner.login);

            Ok(())
        })
    }

    #[test]
    fn add_new_owners_and_delete_old() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id = initialize_crate(&mut conn, "krate").await?;

            // set initial owner details
            update_owners_in_database(
                &mut conn,
                &[CrateOwner {
                    login: "login".into(),
                    avatar: "avatar".into(),
                    kind: OwnerKind::User,
                }],
                crate_id,
            )
            .await?;

            let new_owners: Vec<CrateOwner> = (1..5)
                .map(|i| CrateOwner {
                    login: format!("login{i}"),
                    avatar: format!("avatar{i}"),
                    kind: OwnerKind::User,
                })
                .collect();

            update_owners_in_database(&mut conn, &new_owners, crate_id).await?;

            let all_owners: Vec<String> = sqlx::query!("SELECT login FROM owners order by login")
                .fetch(&mut *conn)
                .map_ok(|row| row.login)
                .try_collect()
                .await?;

            // we still have all owners in the database.
            assert_eq!(
                all_owners,
                vec!["login", "login1", "login2", "login3", "login4"]
            );

            let crate_owners: Vec<String> = sqlx::query!(
                "SELECT o.login
                 FROM owners o, owner_rels r
                 WHERE
                     o.id = r.oid AND
                     r.cid = $1",
                crate_id.0,
            )
            .fetch(&mut *conn)
            .map_ok(|row| row.login)
            .try_collect()
            .await?;

            // the owner-rel is deleted
            assert_eq!(crate_owners, vec!["login1", "login2", "login3", "login4"]);

            Ok(())
        })
    }

    #[test_case("", [])]
    #[test_case(
        r#"
            [features]
            bar = []
        "#,
        [Feature::new("bar".into(), vec![])]
    )]
    #[test_case(
        r#"
            [dependencies]
            bar = { optional = true, path = "bar" }
        "#,
        [Feature::new("bar".into(), vec!["dep:bar".into()])]
    )]
    #[test_case(
        r#"
            [dependencies]
            bar = { optional = true, path = "bar" }
            [features]
            not-bar = ["dep:bar"]
        "#,
        [Feature::new("not-bar".into(), vec!["dep:bar".into()])]
    )]
    fn test_get_features(extra: &str, expected: impl AsRef<[Feature]>) -> Result<()> {
        let dir = tempfile::tempdir()?;

        std::fs::create_dir(dir.path().join("src"))?;
        std::fs::write(dir.path().join("src/lib.rs"), "")?;

        std::fs::create_dir(dir.path().join("bar"))?;
        std::fs::create_dir(dir.path().join("bar/src"))?;
        std::fs::write(dir.path().join("bar/src/lib.rs"), "")?;

        std::fs::write(
            dir.path().join("bar/Cargo.toml"),
            r#"
                [package]
                name = "bar"
                version = "0.0.0"
            "#,
        )?;

        let base = r#"
            [package]
            name = "foo"
            version = "0.0.0"
        "#;

        std::fs::write(dir.path().join("Cargo.toml"), [base, extra].concat())?;
        let metadata = CargoMetadata::load_from_host_path(dir.path())?;
        let features = super::get_features(metadata.root());
        assert_eq!(features, expected.as_ref());

        Ok(())
    }

    #[test]
    fn test_initialize_crate() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;

            let name = "krate";
            let crate_id = initialize_crate(&mut conn, name).await?;

            let id = sqlx::query_scalar!(
                r#"SELECT id as "id: CrateId" FROM crates WHERE name = $1"#,
                name
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(crate_id, id);

            let same_crate_id = initialize_crate(&mut conn, name).await?;
            assert_eq!(crate_id, same_crate_id);

            Ok(())
        })
    }

    #[test]
    fn test_initialize_release() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let name = "krate";
            let version = "0.1.0";
            let crate_id = initialize_crate(&mut conn, name).await?;

            let release_id = initialize_release(&mut conn, crate_id, version).await?;

            let id = sqlx::query_scalar!(
                r#"SELECT id as "id: ReleaseId" FROM releases WHERE crate_id = $1 and version = $2"#,
                crate_id.0,
                version
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(release_id, id);

            let same_release_id = initialize_release(&mut conn, crate_id, version).await?;
            assert_eq!(release_id, same_release_id);

            Ok(())
        })
    }

    #[test]
    fn test_initialize_build() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let name = "krate";
            let version = "0.1.0";
            let crate_id = initialize_crate(&mut conn, name).await?;
            let release_id = initialize_release(&mut conn, crate_id, version).await?;

            let build_id = initialize_build(&mut conn, release_id).await?;

            let id = sqlx::query_scalar!(
                r#"SELECT id as "id: BuildId" FROM builds WHERE rid = $1"#,
                release_id.0
            )
            .fetch_one(&mut *conn)
            .await?;

            assert_eq!(build_id, id);

            let another_build_id = initialize_build(&mut conn, release_id).await?;
            assert_ne!(build_id, another_build_id);

            Ok(())
        })
    }

    #[test]
    fn test_long_crate_name() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;

            let name: String = "krate".repeat(100);
            let crate_id = initialize_crate(&mut conn, &name).await?;

            let db_name = sqlx::query_scalar!("SELECT name FROM crates WHERE id = $1", crate_id.0)
                .fetch_one(&mut *conn)
                .await?;

            assert_eq!(db_name, name);

            Ok(())
        })
    }

    #[test]
    fn test_long_release_version() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;

            let crate_id = initialize_crate(&mut conn, "krate").await?;
            let version: String = "version".repeat(100);
            let release_id = initialize_release(&mut conn, crate_id, &version).await?;

            let db_version =
                sqlx::query_scalar!("SELECT version FROM releases WHERE id = $1", release_id.0)
                    .fetch_one(&mut *conn)
                    .await?;

            assert_eq!(db_version, version);

            Ok(())
        })
    }
}
