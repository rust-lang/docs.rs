use crate::{
    db::types::{BuildStatus, Feature},
    docbuilder::DocCoverage,
    error::Result,
    registry_api::{CrateData, CrateOwner, ReleaseData},
    storage::CompressionAlgorithm,
    utils::MetadataPackage,
    web::crate_details::{latest_release, releases_for_crate},
};
use anyhow::Context;
use futures_util::stream::TryStreamExt;
use serde_json::Value;
use slug::slugify;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::Path,
};
use tracing::{debug, info, instrument};

/// Adds a package into database.
///
/// Package must be built first.
///
/// NOTE: `source_files` refers to the files originally in the crate,
/// not the files generated by rustdoc.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(conn))]
pub(crate) async fn add_package_into_database(
    conn: &mut sqlx::PgConnection,
    metadata_pkg: &MetadataPackage,
    source_dir: &Path,
    default_target: &str,
    source_files: Value,
    doc_targets: Vec<String>,
    registry_data: &ReleaseData,
    has_docs: bool,
    has_examples: bool,
    compression_algorithms: std::collections::HashSet<CompressionAlgorithm>,
    repository_id: Option<i32>,
    archive_storage: bool,
) -> Result<i32> {
    debug!("Adding package into database");
    let crate_id = initialize_package_in_database(conn, metadata_pkg).await?;
    let dependencies = convert_dependencies(metadata_pkg);
    let rustdoc = get_rustdoc(metadata_pkg, source_dir).unwrap_or(None);
    let readme = get_readme(metadata_pkg, source_dir).unwrap_or(None);
    let features = get_features(metadata_pkg);
    let is_library = metadata_pkg.is_library();

    let release_id: i32 = sqlx::query_scalar!(
        "INSERT INTO releases (
            crate_id, version, release_time,
            dependencies, target_name, yanked,
            rustdoc_status, test_status, license, repository_url,
            homepage_url, description, description_long, readme,
            keywords, have_examples, downloads, files,
            doc_targets, is_library,
            documentation_url, default_target, features,
            repository_id, archive_storage
         )
         VALUES (
            $1,  $2,  $3,  $4,  $5,  $6,  $7,  $8,  $9,
            $10, $11, $12, $13, $14, $15, $16, $17, $18,
            $19, $20, $21, $22, $23, $24, $25
         )
         ON CONFLICT (crate_id, version) DO UPDATE
            SET release_time = $3,
                dependencies = $4,
                target_name = $5,
                yanked = $6,
                rustdoc_status = $7,
                test_status = $8,
                license = $9,
                repository_url = $10,
                homepage_url = $11,
                description = $12,
                description_long = $13,
                readme = $14,
                keywords = $15,
                have_examples = $16,
                downloads = $17,
                files = $18,
                doc_targets = $19,
                is_library = $20,
                documentation_url = $21,
                default_target = $22,
                features = $23,
                repository_id = $24,
                archive_storage = $25
         RETURNING id",
        crate_id,
        &metadata_pkg.version,
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
        archive_storage
    )
    .fetch_one(&mut *conn)
    .await?;

    add_keywords_into_database(conn, metadata_pkg, release_id).await?;
    add_compression_into_database(conn, compression_algorithms.into_iter(), release_id).await?;

    update_latest_version_id(&mut *conn, crate_id)
        .await
        .context("couldn't update latest version id")?;

    Ok(release_id)
}

pub async fn update_latest_version_id(conn: &mut sqlx::PgConnection, crate_id: i32) -> Result<()> {
    let releases = releases_for_crate(conn, crate_id).await?;

    sqlx::query!(
        "UPDATE crates
         SET latest_version_id = $2
         WHERE id = $1",
        crate_id,
        latest_release(&releases).map(|release| release.id),
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

async fn crate_id_from_release_id(conn: &mut sqlx::PgConnection, release_id: i32) -> Result<i32> {
    Ok(sqlx::query_scalar!(
        "SELECT crate_id
         FROM releases
         WHERE id = $1",
        release_id,
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[instrument(skip(conn))]
pub(crate) async fn add_doc_coverage(
    conn: &mut sqlx::PgConnection,
    release_id: i32,
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
        &release_id,
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
pub(crate) async fn add_build_into_database(
    conn: &mut sqlx::PgConnection,
    release_id: i32,
    rustc_version: &str,
    docsrs_version: &str,
    build_status: BuildStatus,
) -> Result<i32> {
    debug!("Adding build into database");
    let hostname = hostname::get()?;

    let build_id = sqlx::query_scalar!(
        "INSERT INTO builds (rid, rustc_version, docsrs_version, build_status, build_server)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id",
        release_id,
        rustc_version,
        docsrs_version,
        build_status as BuildStatus,
        hostname.to_str().unwrap_or(""),
    )
    .fetch_one(&mut *conn)
    .await?;

    let crate_id = crate_id_from_release_id(&mut *conn, release_id).await?;
    update_latest_version_id(&mut *conn, crate_id)
        .await
        .context("couldn't update latest version id")?;

    Ok(build_id)
}

async fn initialize_package_in_database(
    conn: &mut sqlx::PgConnection,
    pkg: &MetadataPackage,
) -> Result<i32> {
    if let Some(id) = sqlx::query_scalar!("SELECT id FROM crates WHERE name = $1", pkg.name)
        .fetch_optional(&mut *conn)
        .await?
    {
        Ok(id)
    } else {
        // insert crate into database if it is not exists
        Ok(sqlx::query_scalar!(
            "INSERT INTO crates (name) VALUES ($1) RETURNING id",
            pkg.name,
        )
        .fetch_one(&mut *conn)
        .await?)
    }
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
    if let Some(src_path) = &pkg.targets[0].src_path {
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
    release_id: i32,
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
        release_id,
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
    let crate_id = sqlx::query_scalar!("SELECT id FROM crates WHERE crates.name = $1", name)
        .fetch_one(&mut *conn)
        .await?;

    update_owners_in_database(conn, &registry_data.owners, crate_id).await?;

    Ok(())
}

/// Adds owners into database
async fn update_owners_in_database(
    conn: &mut sqlx::PgConnection,
    owners: &[CrateOwner],
    crate_id: i32,
) -> Result<()> {
    // Update any existing owner data since it is mutable and could have changed since last
    // time we pulled it

    let mut oids: Vec<i32> = Vec::new();

    for owner in owners {
        oids.push(
            sqlx::query_scalar!(
                "INSERT INTO owners (login, avatar)
                 VALUES ($1, $2)
                 ON CONFLICT (login) DO UPDATE
                     SET
                         avatar = EXCLUDED.avatar
                 RETURNING id",
                owner.login,
                owner.avatar
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
        crate_id,
        &oids[..]
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "DELETE FROM owner_rels
         WHERE
            cid = $1 AND
            NOT (oid = ANY($2))",
        crate_id,
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
    release_id: i32,
) -> Result<()>
where
    I: Iterator<Item = CompressionAlgorithm>,
{
    for alg in algorithms {
        sqlx::query!(
            "INSERT INTO compression_rels (release, algorithm)
             VALUES ($1, $2)
             ON CONFLICT DO NOTHING;",
            release_id,
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
    use crate::test::*;
    use crate::utils::{CargoMetadata, MetadataPackage};
    use test_case::test_case;

    #[test]
    fn new_keywords() {
        wrapper(|env| {
            let mut conn = env.db().conn();

            let release_id = env
                .fake_release()
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 1".into(), "kw 2".into()])
                .create()?;

            let kw_r = conn
                .query(
                    "SELECT kw.name,kw.slug
                    FROM keywords as kw
                    INNER JOIN keyword_rels as kwr on kw.id = kwr.kid
                    WHERE kwr.rid = $1
                    ORDER BY kw.name,kw.slug",
                    &[&release_id],
                )?
                .into_iter()
                .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
                .collect::<Vec<_>>();

            assert_eq!(kw_r[0], ("kw 1".into(), "kw-1".into()));
            assert_eq!(kw_r[1], ("kw 2".into(), "kw-2".into()));

            let all_kw = conn
                .query("SELECT slug FROM keywords ORDER BY slug", &[])?
                .into_iter()
                .map(|row| row.get::<_, String>(0))
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
        wrapper(|env| {
            env.fake_release()
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 3".into(), "kw 4".into()])
                .create()?;

            let release_id = env
                .fake_release()
                .name("dummy")
                .version("0.13.0")
                .keywords(vec!["kw 1".into(), "kw 2".into()])
                .create()?;

            let mut conn = env.db().conn();
            let kw_r = conn
                .query(
                    "SELECT kw.name,kw.slug
                    FROM keywords as kw
                    INNER JOIN keyword_rels as kwr on kw.id = kwr.kid
                    WHERE kwr.rid = $1
                    ORDER BY kw.name,kw.slug",
                    &[&release_id],
                )?
                .into_iter()
                .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
                .collect::<Vec<_>>();

            assert_eq!(kw_r[0], ("kw 1".into(), "kw-1".into()));
            assert_eq!(kw_r[1], ("kw 2".into(), "kw-2".into()));

            let all_kw = conn
                .query("SELECT slug FROM keywords ORDER BY slug", &[])?
                .into_iter()
                .map(|row| row.get::<_, String>(0))
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
    fn new_owners() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;

            let crate_id = initialize_package_in_database(
                &mut conn,
                &MetadataPackage {
                    ..Default::default()
                },
            )
            .await?;

            let owner1 = CrateOwner {
                avatar: "avatar".into(),
                login: "login".into(),
            };

            update_owners_in_database(&mut conn, &[owner1.clone()], crate_id).await?;

            let owner_def = sqlx::query!(
                "SELECT login, avatar
                FROM owners"
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_def.login, owner1.login);
            assert_eq!(owner_def.avatar, owner1.avatar);

            let owner_rel = sqlx::query!(
                "SELECT o.login
                FROM owners o, owner_rels r
                WHERE
                    o.id = r.oid AND
                    r.cid = $1",
                crate_id
            )
            .fetch_one(&mut *conn)
            .await?;
            assert_eq!(owner_rel.login, owner1.login);

            Ok(())
        })
    }

    #[test]
    fn update_owner_detais() {
        async_wrapper(|env| async move {
            let mut conn = env.async_db().await.async_conn().await;
            let crate_id =
                initialize_package_in_database(&mut conn, &MetadataPackage::default()).await?;

            // set initial owner details
            update_owners_in_database(
                &mut conn,
                &[CrateOwner {
                    login: "login".into(),
                    avatar: "avatar".into(),
                }],
                crate_id,
            )
            .await?;

            let updated_owner = CrateOwner {
                login: "login".into(),
                avatar: "avatar2".into(),
            };
            update_owners_in_database(&mut conn, &[updated_owner.clone()], crate_id).await?;

            let owner_def = sqlx::query!("SELECT login, avatar FROM owners")
                .fetch_one(&mut *conn)
                .await?;
            assert_eq!(owner_def.login, updated_owner.login);
            assert_eq!(owner_def.avatar, updated_owner.avatar);

            let owner_rel = sqlx::query!(
                "SELECT o.login
                FROM owners o, owner_rels r
                WHERE
                    o.id = r.oid AND
                    r.cid = $1",
                crate_id
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
            let crate_id = initialize_package_in_database(
                &mut conn,
                &MetadataPackage {
                    ..Default::default()
                },
            )
            .await?;

            // set initial owner details
            update_owners_in_database(
                &mut conn,
                &[CrateOwner {
                    login: "login".into(),
                    avatar: "avatar".into(),
                }],
                crate_id,
            )
            .await?;

            let new_owners: Vec<CrateOwner> = (1..5)
                .map(|i| CrateOwner {
                    login: format!("login{i}"),
                    avatar: format!("avatar{i}"),
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
                crate_id,
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
}
