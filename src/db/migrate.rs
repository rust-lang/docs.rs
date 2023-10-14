//! Database migrations

use postgres::{Client, Error as PostgresError, Transaction};
use schemamama::{Migration, Migrator, Version};
use schemamama_postgres::{PostgresAdapter, PostgresMigration};
use tracing::{event, Level};

/// Creates a new PostgresMigration from upgrade and downgrade closures.
/// Directly using `migration` is only needed for data-migrations or more
/// complex schema-migrations.
///
/// Example:
///
/// ```no_compile
/// let my_migration = migration!(100,
///                               "migrate data",
///                               |transaction: &mut Transaction| -> Result<(), PostgresError> {
///                                   // upgrade logic here
///                               },
///                               |transaction: &mut Transaction| -> Result<(), PostgresError> {
///                                   // downgrade logic here
///                               } );
/// ```
const LOG_LEVEL: Level = if cfg!(test) {
    Level::TRACE
} else {
    Level::INFO
};
macro_rules! migration {
    ($context:expr, $version:expr, $description:expr, $up_func:expr, $down_func:expr $(,)?) => {{
        struct Amigration;
        impl Migration for Amigration {
            fn version(&self) -> Version {
                $version
            }
            fn description(&self) -> String {
                $description.to_owned()
            }
        }

        impl PostgresMigration for Amigration {
            fn up(&self, transaction: &mut Transaction) -> Result<(), PostgresError> {
                event!(
                    LOG_LEVEL,
                    "Applying migration {}: {}",
                    self.version(),
                    self.description()
                );

                #[allow(clippy::redundant_closure_call)]
                $up_func(transaction)
            }
            fn down(&self, transaction: &mut Transaction) -> Result<(), PostgresError> {
                event!(
                    LOG_LEVEL,
                    "Removing migration {}: {}",
                    self.version(),
                    self.description()
                );
                #[allow(clippy::redundant_closure_call)]
                $down_func(transaction)
            }
        }
        Box::new(Amigration)
    }};
}

/// Creates a new PostgresMigration from upgrade and downgrade queries.
/// Downgrade query should return database to previous state.
///
/// Example:
///
/// ```no_compile
/// let my_migration = sql_migration!(100,
///                               "Create test table",
///                               "CREATE TABLE test ( id SERIAL);",
///                               "DROP TABLE test;");
/// ```
macro_rules! sql_migration {
    ($context:expr, $version:expr, $description:expr, $up:expr, $down:expr $(,)?) => {{
        migration!(
            $context,
            $version,
            $description,
            |transaction: &mut Transaction| -> Result<(), PostgresError> {
                transaction.batch_execute($up).map(|_| ())
            },
            |transaction: &mut Transaction| -> Result<(), PostgresError> {
                transaction.batch_execute($down).map(|_| ())
            }
        )
    }};
}

pub fn migrate(version: Option<Version>, conn: &mut Client) -> crate::error::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS database_versions (version BIGINT PRIMARY KEY);",
        &[],
    )?;
    let mut adapter = PostgresAdapter::new(conn);
    adapter.set_metadata_table("database_versions");

    let mut migrator = Migrator::new(adapter);

    let migrations: Vec<Box<dyn PostgresMigration>> = vec![
        sql_migration!(
            context,
            // version
            1,
            // description
            "Initial database schema",
            // upgrade query
            "CREATE TABLE crates (
                 id SERIAL PRIMARY KEY,
                 name VARCHAR(255) UNIQUE NOT NULL,
                 latest_version_id INT DEFAULT 0,
                 versions JSON DEFAULT '[]',
                 downloads_total INT DEFAULT 0,
                 github_description VARCHAR(1024),
                 github_stars INT DEFAULT 0,
                 github_forks INT DEFAULT 0,
                 github_issues INT DEFAULT 0,
                 github_last_commit TIMESTAMP,
                 github_last_update TIMESTAMP,
                 content tsvector
             );
             CREATE TABLE releases (
                 id SERIAL PRIMARY KEY,
                 crate_id INT NOT NULL REFERENCES crates(id),
                 version VARCHAR(100),
                 release_time TIMESTAMP,
                 dependencies JSON,
                 target_name VARCHAR(255),
                 yanked BOOL DEFAULT FALSE,
                 is_library BOOL DEFAULT TRUE,
                 build_status BOOL DEFAULT FALSE,
                 rustdoc_status BOOL DEFAULT FALSE,
                 test_status BOOL DEFAULT FALSE,
                 license VARCHAR(100),
                 repository_url VARCHAR(255),
                 homepage_url VARCHAR(255),
                 documentation_url VARCHAR(255),
                 description VARCHAR(1024),
                 description_long VARCHAR(51200),
                 readme VARCHAR(51200),
                 authors JSON,
                 keywords JSON,
                 have_examples BOOL DEFAULT FALSE,
                 downloads INT DEFAULT 0,
                 files JSON,
                 doc_targets JSON DEFAULT '[]',
                 doc_rustc_version VARCHAR(100) NOT NULL,
                 default_target VARCHAR(100),
                 UNIQUE (crate_id, version)
             );
             CREATE TABLE authors (
                 id SERIAL PRIMARY KEY,
                 name VARCHAR(255),
                 email VARCHAR(255),
                 slug VARCHAR(255) UNIQUE NOT NULL
             );
             CREATE TABLE author_rels (
                 rid INT REFERENCES releases(id),
                 aid INT REFERENCES authors(id),
                 UNIQUE(rid, aid)
             );
             CREATE TABLE keywords (
                 id SERIAL PRIMARY KEY,
                 name VARCHAR(255),
                 slug VARCHAR(255) NOT NULL UNIQUE
             );
             CREATE TABLE keyword_rels (
                 rid INT REFERENCES releases(id),
                 kid INT REFERENCES keywords(id),
                 UNIQUE(rid, kid)
             );
             CREATE TABLE owners (
                 id SERIAL PRIMARY KEY,
                 login VARCHAR(255) NOT NULL UNIQUE,
                 avatar VARCHAR(255),
                 name VARCHAR(255),
                 email VARCHAR(255)
             );
             CREATE TABLE owner_rels (
                 cid INT REFERENCES releases(id),
                 oid INT REFERENCES owners(id),
                 UNIQUE(cid, oid)
             );
             CREATE TABLE builds (
                 id SERIAL,
                 rid INT NOT NULL REFERENCES releases(id),
                 rustc_version VARCHAR(100) NOT NULL,
                 cratesfyi_version VARCHAR(100) NOT NULL,
                 build_status BOOL NOT NULL,
                 build_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 output TEXT
             );
             CREATE TABLE queue (
                 id SERIAL,
                 name VARCHAR(255),
                 version VARCHAR(100),
                 attempt INT DEFAULT 0,
                 date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 UNIQUE(name, version)
             );
             CREATE TABLE files (
                 path VARCHAR(4096) NOT NULL PRIMARY KEY,
                 mime VARCHAR(100) NOT NULL,
                 date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 date_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 content BYTEA
             );
             CREATE TABLE config (
                 name VARCHAR(100) NOT NULL PRIMARY KEY,
                 value JSON NOT NULL
             );
             CREATE INDEX ON releases (release_time DESC);
             CREATE INDEX content_idx ON crates USING gin(content);",
            // downgrade query
            "DROP TABLE authors, author_rels, keyword_rels, keywords, owner_rels,
                        owners, releases, crates, builds, queue, files, config;"
        ),
        sql_migration!(
            context,
            // version
            2,
            // description
            "Added priority column to build queue",
            // upgrade query
            "ALTER TABLE queue ADD COLUMN priority INT DEFAULT 0;",
            // downgrade query
            "ALTER TABLE queue DROP COLUMN priority;"
        ),
        sql_migration!(
            context,
            // version
            3,
            // description
            "Added sandbox_overrides table",
            // upgrade query
            "CREATE TABLE sandbox_overrides (
                 crate_name VARCHAR NOT NULL PRIMARY KEY,
                 max_memory_bytes INTEGER,
                 timeout_seconds INTEGER
             );",
            // downgrade query
            "DROP TABLE sandbox_overrides;"
        ),
        sql_migration!(
            context,
            4,
            "Make more fields not null",
            "ALTER TABLE releases ALTER COLUMN release_time SET NOT NULL,
                                  ALTER COLUMN yanked SET NOT NULL,
                                  ALTER COLUMN downloads SET NOT NULL",
            "ALTER TABLE releases ALTER COLUMN release_time DROP NOT NULL,
                                  ALTER COLUMN yanked DROP NOT NULL,
                                  ALTER COLUMN downloads DROP NOT NULL"
        ),
        sql_migration!(
            context,
            // version
            5,
            // description
            "Make target_name non-nullable",
            // upgrade query
            "ALTER TABLE releases ALTER COLUMN target_name SET NOT NULL",
            // downgrade query
            "ALTER TABLE releases ALTER COLUMN target_name DROP NOT NULL",
        ),
        sql_migration!(
            context,
            // version
            6,
            // description
            "Added blacklisted_crates table",
            // upgrade query
            "CREATE TABLE blacklisted_crates (
                 crate_name VARCHAR NOT NULL PRIMARY KEY
             );",
            // downgrade query
            "DROP TABLE blacklisted_crates;"
        ),
        sql_migration!(
            context,
            // version
            7,
            // description
            "Allow memory limits of more than 2 GB",
            // upgrade query
            "ALTER TABLE sandbox_overrides ALTER COLUMN max_memory_bytes TYPE BIGINT;",
            // downgrade query
            "
                ALTER TABLE sandbox_overrides
                    ALTER COLUMN max_memory_bytes
                    TYPE INTEGER
                    USING least(max_memory_bytes, 2147483647);
            "
        ),
        sql_migration!(
            context,
            // version
            8,
            // description
            "Make default_target non-nullable",
            // upgrade query
            "UPDATE releases SET default_target = 'x86_64-unknown-linux-gnu' WHERE default_target IS NULL;
             ALTER TABLE releases ALTER COLUMN default_target SET NOT NULL",
            // downgrade query
            "ALTER TABLE releases ALTER COLUMN default_target DROP NOT NULL;
             ALTER TABLE releases ALTER COLUMN default_target DROP DEFAULT",
        ),
        sql_migration!(
            context,
            // version
            9,
            // description
            "Allow max number of targets to be overridden",
            // upgrade query
            "ALTER TABLE sandbox_overrides ADD COLUMN max_targets INT;",
            // downgrade query
            "ALTER TABLE sandbox_overrides DROP COLUMN max_targets;"
        ),
        sql_migration!(
            context,
            // version
            10,
            // description
            "Add function to normalize underscores in crate names",
            // upgrade query
            "
                CREATE FUNCTION normalize_crate_name(VARCHAR)
                RETURNS VARCHAR
                AS $$
                    SELECT LOWER(REPLACE($1, '_', '-'));
                $$ LANGUAGE SQL;

                CREATE UNIQUE INDEX crates_normalized_name_idx
                    ON crates (normalize_crate_name(name));
            ",
            // downgrade query
            "
                DROP INDEX crates_normalized_name_idx;
                DROP FUNCTION normalize_crate_name;
            "
        ),
        sql_migration!(
            context,
            // version
            11,
            // description
            "Allow crates to be given a different default priority",
            // upgrade query
            "CREATE TABLE crate_priorities (
                pattern VARCHAR NOT NULL UNIQUE,
                priority INTEGER NOT NULL
            );",
            // downgrade query
            "DROP TABLE crate_priorities;",
        ),
        sql_migration!(
            context,
            // version
            12,
            // description
            "Mark doc_targets non-nullable (it has a default of empty array anyway)",
            // upgrade query
            "
                ALTER TABLE releases ALTER COLUMN doc_targets SET NOT NULL;
            ",
            // downgrade query
            "
                ALTER TABLE releases ALTER COLUMN doc_targets DROP NOT NULL;
            "
        ),
        sql_migration!(
            context,
            // version
            13,
            // description
            "Remove the content column and releases column",
            // upgrade query
            "ALTER TABLE crates
             DROP COLUMN content,
             DROP COLUMN versions;",
            // downgrade query
            "ALTER TABLE crates
             ADD COLUMN content tsvector,
             ADD COLUMN versions JSON DEFAULT '[]';"
        ),
        sql_migration!(
            context,
            // version
            14,
            // description
            "Add compression",
            // upgrade query
            "
            -- NULL indicates the file was not compressed.
            -- There is no meaning assigned to the compression id in the database itself,
            -- it is instead interpreted by the application.
            ALTER TABLE files ADD COLUMN compression INT;
            -- many to many table between releases and compression
            -- stores the set of all compression algorithms used in the release files
            CREATE TABLE compression_rels (
                release INT NOT NULL REFERENCES releases(id),
                algorithm INT,
                -- make sure we don't store duplicates by accident
                UNIQUE(release, algorithm)
            );",
            // downgrade query
            "DROP TABLE compression_rels;
             ALTER TABLE files DROP COLUMN compression;"
        ),
        sql_migration!(
            context,
            // version
            15,
            // description
            "Fix owner_rels.cid foreign key reference",
            // upgrade query
            "
            ALTER TABLE owner_rels DROP CONSTRAINT owner_rels_cid_fkey;
            ALTER TABLE owner_rels ADD FOREIGN KEY (cid) REFERENCES crates(id);
            ",
            // downgrade query
            "
            -- Nope, this is a pure database fix, no going back.
            "
        ),
        sql_migration!(
            context,
            // version
            16,
            // description
            "Create new table for doc coverage",
            // upgrade query
            "
            CREATE TABLE doc_coverage (
                release_id INT UNIQUE REFERENCES releases(id),
                total_items INT,
                documented_items INT
            );
            ",
            // downgrade query
            "DROP TABLE doc_coverage;"
        ),
        sql_migration!(
            context,
            // version
            17,
            // description
            "Make many more fields non-null",
            // upgrade query
            "
            ALTER TABLE queue ALTER COLUMN name SET NOT NULL;
            ALTER TABLE queue ALTER COLUMN version SET NOT NULL;
            ALTER TABLE queue ALTER COLUMN priority SET NOT NULL;
            ALTER TABLE queue ALTER COLUMN attempt SET NOT NULL;
            ALTER TABLE doc_coverage ALTER COLUMN release_id SET NOT NULL;
            ALTER TABLE releases ALTER COLUMN version SET NOT NULL;
            ALTER TABLE releases ALTER COLUMN rustdoc_status SET NOT NULL;
            ALTER TABLE releases ALTER COLUMN build_status SET NOT NULL;
            ALTER TABLE releases ALTER COLUMN have_examples SET NOT NULL;
            ALTER TABLE releases ALTER COLUMN is_library SET NOT NULL;
            ALTER TABLE authors ALTER COLUMN name SET NOT NULL;
            ALTER TABLE owners ALTER COLUMN avatar SET NOT NULL;
            ALTER TABLE owners ALTER COLUMN name SET NOT NULL;
            ALTER TABLE crates ALTER COLUMN github_stars SET NOT NULL;
            ",
            // downgrade query
            "
            ALTER TABLE queue ALTER COLUMN name DROP NOT NULL;
            ALTER TABLE queue ALTER COLUMN version DROP NOT NULL;
            ALTER TABLE queue ALTER COLUMN priority DROP NOT NULL;
            ALTER TABLE queue ALTER COLUMN attempt DROP NOT NULL;
            ALTER TABLE doc_coverage ALTER COLUMN release_id DROP NOT NULL;
            ALTER TABLE releases ALTER COLUMN version DROP NOT NULL;
            ALTER TABLE releases ALTER COLUMN rustdoc_status DROP NOT NULL;
            ALTER TABLE releases ALTER COLUMN build_status DROP NOT NULL;
            ALTER TABLE releases ALTER COLUMN have_examples DROP NOT NULL;
            ALTER TABLE releases ALTER COLUMN is_library DROP NOT NULL;
            ALTER TABLE authors ALTER COLUMN name DROP NOT NULL;
            ALTER TABLE owners ALTER COLUMN avatar DROP NOT NULL;
            ALTER TABLE owners ALTER COLUMN name DROP NOT NULL;
            ALTER TABLE crates ALTER COLUMN github_stars DROP NOT NULL;
            "
        ),
        sql_migration!(
            context,
            // version
            18,
            // description
            "Add more information into doc coverage",
            // upgrade query
            "
            ALTER TABLE doc_coverage
                ADD COLUMN total_items_needing_examples INT,
                ADD COLUMN items_with_examples INT;
            ",
            // downgrade query
            "
            ALTER TABLE doc_coverage
                DROP COLUMN total_items_needing_examples,
                DROP COLUMN items_with_examples;
            "
        ),
        sql_migration!(
            context,
            // version
            19,
            // description
            "Add features that are available for given release",
            // upgrade query
            "
                CREATE TYPE feature AS (name TEXT, subfeatures TEXT[]);
                ALTER TABLE releases ADD COLUMN features feature[];
            ",
            // downgrade query
            "
                ALTER TABLE releases DROP COLUMN features;
                DROP TYPE feature;
            "
        ),
        sql_migration!(
            context,
            20,
            // description
            "Support alternative registries",
            // upgrade query
            "
                ALTER TABLE queue ADD COLUMN registry TEXT DEFAULT NULL;
            ",
            // downgrade query
            "
                ALTER TABLE queue DROP COLUMN registry;
            "
        ),
        sql_migration!(
            context,
            21,
            // description
            "Add mark for features that are derived from optional dependencies",
            // upgrade query
            "
                ALTER TYPE feature ADD ATTRIBUTE optional_dependency BOOL;
            ",
            // downgrade query
            "
                 ALTER TYPE feature DROP ATTRIBUTE optional_dependency;
            "
        ),
        sql_migration!(
            context,
            22,
            // description
            "Add the github_repositories table to contain GitHub information",
            // upgrade query
            "
                CREATE TABLE github_repos (
                    id VARCHAR PRIMARY KEY NOT NULL,
                    name VARCHAR NOT NULL,
                    description VARCHAR,
                    last_commit TIMESTAMP,
                    stars INT NOT NULL,
                    forks INT NOT NULL,
                    issues INT NOT NULL,
                    updated_at TIMESTAMP NOT NULL
                );

                ALTER TABLE releases ADD COLUMN github_repo VARCHAR
                    REFERENCES github_repos(id) ON DELETE SET NULL;
            ",
            // downgrade query
            "
                ALTER TABLE releases DROP COLUMN github_repo;
                DROP TABLE github_repos;
            "
        ),
        sql_migration!(
            context,
            23,
            // description
            "Delete old GitHub stats",
            // upgrade query
            "
                ALTER TABLE crates
                    DROP COLUMN github_description,
                    DROP COLUMN github_stars,
                    DROP COLUMN github_forks,
                    DROP COLUMN github_issues,
                    DROP COLUMN github_last_commit,
                    DROP COLUMN github_last_update;
            ",
            // downgrade query
            "
                ALTER TABLE crates
                    ADD COLUMN github_description VARCHAR(1024),
                    ADD COLUMN github_stars INTEGER NOT NULL DEFAULT 0,
                    ADD COLUMN github_forks INTEGER DEFAULT 0,
                    ADD COLUMN github_issues INTEGER DEFAULT 0,
                    ADD COLUMN github_last_commit TIMESTAMP,
                    ADD COLUMN github_last_update TIMESTAMP;
            "
        ),
        sql_migration!(
            context,
            24,
            "drop unused `date_added` columns",
            // upgrade
            "ALTER TABLE queue DROP COLUMN IF EXISTS date_added;
             ALTER TABLE files DROP COLUMN IF EXISTS date_added;",
             // downgrade
             "ALTER TABLE queue ADD COLUMN date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP;
              ALTER TABLE files ADD COLUMN date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP;",
        ),
        sql_migration!(
            context,
            25,
            "migrate timestamp to be timezone aware",
            // upgrade
            "
                ALTER TABLE builds
                    ALTER build_time TYPE timestamptz USING build_time AT TIME ZONE 'UTC';

                ALTER TABLE files
                    ALTER date_updated TYPE timestamptz USING date_updated AT TIME ZONE 'UTC';

                ALTER TABLE github_repos
                    ALTER updated_at TYPE timestamptz USING updated_at AT TIME ZONE 'UTC',
                    ALTER last_commit TYPE timestamptz USING last_commit AT TIME ZONE 'UTC';

                ALTER TABLE releases
                    ALTER release_time TYPE timestamptz USING release_time AT TIME ZONE 'UTC';
            ",
            // downgrade
            "
                ALTER TABLE builds
                    ALTER build_time TYPE timestamp USING build_time AT TIME ZONE 'UTC';

                ALTER TABLE files
                    ALTER date_updated TYPE timestamp USING date_updated AT TIME ZONE 'UTC';

                ALTER TABLE github_repos
                    ALTER updated_at TYPE timestamp USING updated_at AT TIME ZONE 'UTC',
                    ALTER last_commit TYPE timestamp USING last_commit AT TIME ZONE 'UTC';

                ALTER TABLE releases
                    ALTER release_time TYPE timestamp USING release_time AT TIME ZONE 'UTC';
            ",
        ),
        sql_migration!(
            context,
            26,
            "create indexes for crates, github_repos and releases",
            // upgrade
            "
            CREATE INDEX crates_latest_version_idx ON crates (latest_version_id);
            CREATE INDEX releases_github_repo_idx ON releases (github_repo);
            CREATE INDEX github_repos_stars_idx ON github_repos(stars DESC);
            ",
            // downgrade
            "
            DROP INDEX crates_latest_version_idx;
            DROP INDEX releases_github_repo_idx;
            DROP INDEX github_repos_stars_idx;
            ",
        ),
        sql_migration!(
            context,
            27,
            "delete the authors and author_rels",
            // upgrade
            "
            DROP TABLE authors, author_rels;
            ALTER TABLE releases DROP COLUMN authors;
            ",
            // downgrade
            "
            CREATE TABLE authors (
                id SERIAL PRIMARY KEY,
                name VARCHAR(255),
                email VARCHAR(255),
                slug VARCHAR(255) UNIQUE NOT NULL
            );
            CREATE TABLE author_rels (
                rid INT REFERENCES releases(id),
                aid INT REFERENCES authors(id),
                UNIQUE(rid, aid)
            );
            ALTER TABLE releases ADD COLUMN authors JSON;
            ",
        ),
        sql_migration!(
            context,
            28,
            // description
            "Add gitlab handling: creation of the new repositories table which replaces and extend \
             github_repos",
            // upgrade query
            "
                CREATE TABLE repositories (
                    id SERIAL PRIMARY KEY,
                    host VARCHAR NOT NULL,
                    host_id VARCHAR NOT NULL,
                    name VARCHAR NOT NULL,
                    description VARCHAR,
                    last_commit TIMESTAMPTZ,
                    stars INT NOT NULL,
                    forks INT NOT NULL,
                    issues INT NOT NULL,
                    updated_at TIMESTAMPTZ NOT NULL,
                    UNIQUE (host, host_id)
                );

                ALTER TABLE releases ADD COLUMN repository_id INTEGER
                    REFERENCES repositories(id) ON DELETE SET NULL;

                INSERT INTO repositories(host, host_id, name, description, last_commit, stars, forks, issues, updated_at)
                    SELECT 'github.com', id, name, description, last_commit, stars, forks, issues, updated_at
                    FROM github_repos;

                UPDATE releases
                    SET repository_id = repositories.id
                FROM repositories
                WHERE releases.github_repo IS NOT NULL AND repositories.host_id = releases.github_repo;

                DROP INDEX releases_github_repo_idx;
                DROP INDEX github_repos_stars_idx;

                CREATE INDEX releases_repo_idx ON releases(repository_id);
                CREATE INDEX repos_stars_idx ON repositories(stars DESC);

                ALTER TABLE releases
                    DROP COLUMN github_repo;

                DROP TABLE github_repos;
            ",
            // downgrade query
            "
                CREATE TABLE github_repos (
                    id VARCHAR PRIMARY KEY NOT NULL,
                    name VARCHAR NOT NULL,
                    description VARCHAR,
                    last_commit TIMESTAMPTZ,
                    stars INT NOT NULL,
                    forks INT NOT NULL,
                    issues INT NOT NULL,
                    updated_at TIMESTAMPTZ NOT NULL
                );

                ALTER TABLE releases ADD COLUMN github_repo VARCHAR
                    REFERENCES github_repos(id) ON DELETE SET NULL;

                INSERT INTO github_repos(id, name, description, last_commit, stars, forks, issues, updated_at)
                    SELECT host_id, name, description, last_commit, stars, forks, issues, updated_at
                    FROM repositories WHERE repositories.host = 'github.com';

                UPDATE releases
                    SET github_repo = (
                        SELECT host_id
                        FROM repositories
                        WHERE
                            repositories.host = 'github.com' AND
                            repositories.id = releases.repository_id
                    )
                WHERE
                    repository_id IS NOT NULL;

                DROP INDEX releases_repo_idx;
                DROP INDEX repos_stars_idx;

                CREATE INDEX releases_github_repo_idx ON releases (github_repo);
                CREATE INDEX github_repos_stars_idx ON github_repos(stars DESC);

                ALTER TABLE releases
                    DROP COLUMN repository_id;

                DROP TABLE repositories;
            ",
        ),
        sql_migration!(
            context, 29, "Rename cratesfyi_version to docsrs_version",
            "ALTER TABLE builds RENAME COLUMN cratesfyi_version TO docsrs_version",
            "ALTER TABLE builds RENAME COLUMN docsrs_version TO cratesfyi_version",
        ),
        sql_migration!(
            context, 30, "add archive-storage marker for releases",
            "ALTER TABLE releases ADD COLUMN archive_storage BOOL NOT NULL DEFAULT FALSE;",
            "ALTER TABLE releases DROP COLUMN archive_storage;",
        ),
        sql_migration!(
            context, 31, "add index on builds.build_time",
            "CREATE INDEX builds_build_time_idx ON builds (build_time DESC);",
            "DROP INDEX builds_build_time_idx;",
        ),
        migration!(
            context,
            32,
            "update latest_version_id on all crates",
            |transaction: &mut Transaction| -> Result<(), PostgresError> {
                use crate::web::crate_details::CrateDetails;
                let rows = transaction.query(
                    "SELECT crates.name, max(releases.version) as max_version_id
                     FROM crates
                     INNER JOIN releases ON crates.id = releases.crate_id
                     GROUP BY crates.name",
                    &[],
                )?;

                let update_version_query = transaction.prepare(
                    "UPDATE crates
                     SET latest_version_id = $2
                     WHERE id = $1",
                )?;

                for row in rows.into_iter() {
                    if let Some(details) = CrateDetails::new(transaction, row.get(0), row.get(1), row.get(1), None)
                            .expect("error when fetching crate details") {
                        transaction.execute(
                            &update_version_query,
                            &[&details.crate_id, &details.latest_release().id],
                        )?;
                    }
                }

                Ok(())
            },
            |transaction: &mut Transaction| -> Result<(), PostgresError> {
                transaction
                    .execute(
                        "UPDATE crates
                         SET latest_version_id = (
                             SELECT max(id)
                             FROM releases
                             WHERE releases.crate_id = crates.id
                         )",
                        &[],
                    )
                    .map(|_| ())
            }
        ),
        sql_migration!(
            context, 33, "add hostname to build-table",
            "ALTER TABLE builds ADD COLUMN build_server TEXT NOT NULL DEFAULT '';",
            "ALTER TABLE builds DROP COLUMN build_server;",
        ),
        sql_migration!(
            context, 34, "add index on builds.rid",
            "CREATE INDEX builds_release_id_idx ON builds (rid);",
            "DROP INDEX builds_release_id_idx;",
        ),
        sql_migration!(
            context, 35, "add public visibility to files table",
            "ALTER TABLE files ADD COLUMN public BOOL NOT NULL DEFAULT FALSE;",
            "ALTER TABLE files DROP COLUMN public;"
        ),
        sql_migration!(
            context, 36, "remove email & name columns from owner table",
            "
            ALTER TABLE owners DROP COLUMN email;
            ALTER TABLE owners DROP COLUMN name;
            ",
            "
            ALTER TABLE owners ADD COLUMN email VARCHAR(255);
            ALTER TABLE owners ADD COLUMN name VARCHAR(255);
            ",
        ),
        sql_migration!(
            context, 37, "add cdn-invalidation-queue table",
            "
            CREATE TABLE cdn_invalidation_queue (
                id BIGSERIAL,
                crate VARCHAR(255) NOT NULL,
                cdn_distribution_id VARCHAR(255) NOT NULL,
                path_pattern text NOT NULL,
                queued TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
                created_in_cdn TIMESTAMP WITH TIME ZONE,
                cdn_reference VARCHAR(255)
            );
            CREATE INDEX cdn_invalidation_queue_crate_idx ON cdn_invalidation_queue (crate);
            CREATE INDEX cdn_invalidation_queue_cdn_reference_idx ON cdn_invalidation_queue (cdn_reference);
            CREATE INDEX cdn_invalidation_queue_created_in_cdn_idx ON cdn_invalidation_queue (created_in_cdn);
            ",
            "
            DROP INDEX cdn_invalidation_queue_crate_idx;
            DROP INDEX cdn_invalidation_queue_cdn_reference_idx;
            DROP INDEX cdn_invalidation_queue_created_in_cdn_idx;
            DROP TABLE cdn_invalidation_queue;
            "
        ),
        sql_migration!(
            context,
            38,
            "Remove mark for features that are derived from optional dependencies",
            "ALTER TYPE feature DROP ATTRIBUTE optional_dependency;",
            "ALTER TYPE feature ADD ATTRIBUTE optional_dependency BOOL;"
        ),
        sql_migration!(
            context,
            39,
            "Added last_attempt column to build queue",
            // upgrade query
            "ALTER TABLE queue ADD COLUMN last_attempt TIMESTAMP WITH TIME ZONE;",
            // downgrade query
            "ALTER TABLE queue DROP COLUMN last_attempt;"
        ),
    ];

    for migration in migrations {
        migrator.register(migration);
    }

    if let Some(version) = version {
        if version > migrator.current_version()?.unwrap_or(0) {
            migrator.up(Some(version))?;
        } else {
            migrator.down(Some(version))?;
        }
    } else {
        migrator.up(version)?;
    }

    Ok(())
}
