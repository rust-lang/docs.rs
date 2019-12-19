//! Database migrations

use error::Result as CratesfyiResult;
use postgres::{Connection, transaction::Transaction, Error as PostgresError};
use schemamama::{Migration, Migrator, Version};
use schemamama_postgres::{PostgresAdapter, PostgresMigration};
use std::rc::Rc;


enum ApplyMode {
    Permanent,
    Temporary,
}

struct MigrationContext {
    apply_mode: ApplyMode,
}

impl MigrationContext {
    fn format_query(&self, query: &str) -> String {
        query.replace("{create_table}", match self.apply_mode {
            ApplyMode::Permanent => "CREATE TABLE",
            ApplyMode::Temporary => "CREATE TEMPORARY TABLE",
        })
    }
}


/// Creates a new PostgresMigration from upgrade and downgrade queries.
/// Downgrade query should return database to previous state.
///
/// Example:
///
/// ```
/// let my_migration = migration!(100,
///                               "Create test table",
///                               "{create_table} test ( id SERIAL);",
///                               "DROP TABLE test;");
/// ```
macro_rules! migration {
    ($context:expr, $version:expr, $description:expr, $up:expr, $down:expr $(,)?) => {{
        struct Amigration {
            ctx: Rc<MigrationContext>,
        };
        impl Migration for Amigration {
            fn version(&self) -> Version {
                $version
            }
            fn description(&self) -> String {
                $description.to_owned()
            }
        }
        impl PostgresMigration for Amigration {
            fn up(&self, transaction: &Transaction) -> Result<(), PostgresError> {
                info!("Applying migration {}: {}", self.version(), self.description());
                transaction.batch_execute(&self.ctx.format_query($up)).map(|_| ())
            }
            fn down(&self, transaction: &Transaction) -> Result<(), PostgresError> {
                info!("Removing migration {}: {}", self.version(), self.description());
                transaction.batch_execute(&self.ctx.format_query($down)).map(|_| ())
            }
        }
        Box::new(Amigration { ctx: $context })
    }};
}


pub fn migrate(version: Option<Version>, conn: &Connection) -> CratesfyiResult<()> {
    migrate_inner(version, conn, ApplyMode::Permanent)
}

pub fn migrate_temporary(version: Option<Version>, conn: &Connection) -> CratesfyiResult<()> {
    migrate_inner(version, conn, ApplyMode::Temporary)
}

fn migrate_inner(version: Option<Version>, conn: &Connection, apply_mode: ApplyMode) -> CratesfyiResult<()> {
    let context = Rc::new(MigrationContext { apply_mode });

    conn.execute(
        &context.format_query(
            "{create_table} IF NOT EXISTS database_versions (version BIGINT PRIMARY KEY);"
        ),
        &[],
    )?;
    let adapter = PostgresAdapter::with_metadata_table(conn, "database_versions");

    let mut migrator = Migrator::new(adapter);

    let migrations: Vec<Box<dyn PostgresMigration>> = vec![
        migration!(
            context.clone(),
            // version
            1,
            // description
            "Initial database schema",
            // upgrade query
            "{create_table} crates (
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
             {create_table} releases (
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
             {create_table} authors (
                 id SERIAL PRIMARY KEY,
                 name VARCHAR(255),
                 email VARCHAR(255),
                 slug VARCHAR(255) UNIQUE NOT NULL
             );
             {create_table} author_rels (
                 rid INT REFERENCES releases(id),
                 aid INT REFERENCES authors(id),
                 UNIQUE(rid, aid)
             );
             {create_table} keywords (
                 id SERIAL PRIMARY KEY,
                 name VARCHAR(255),
                 slug VARCHAR(255) NOT NULL UNIQUE
             );
             {create_table} keyword_rels (
                 rid INT REFERENCES releases(id),
                 kid INT REFERENCES keywords(id),
                 UNIQUE(rid, kid)
             );
             {create_table} owners (
                 id SERIAL PRIMARY KEY,
                 login VARCHAR(255) NOT NULL UNIQUE,
                 avatar VARCHAR(255),
                 name VARCHAR(255),
                 email VARCHAR(255)
             );
             {create_table} owner_rels (
                 cid INT REFERENCES releases(id),
                 oid INT REFERENCES owners(id),
                 UNIQUE(cid, oid)
             );
             {create_table} builds (
                 id SERIAL,
                 rid INT NOT NULL REFERENCES releases(id),
                 rustc_version VARCHAR(100) NOT NULL,
                 cratesfyi_version VARCHAR(100) NOT NULL,
                 build_status BOOL NOT NULL,
                 build_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 output TEXT
             );
             {create_table} queue (
                 id SERIAL,
                 name VARCHAR(255),
                 version VARCHAR(100),
                 attempt INT DEFAULT 0,
                 date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 UNIQUE(name, version)
             );
             {create_table} files (
                 path VARCHAR(4096) NOT NULL PRIMARY KEY,
                 mime VARCHAR(100) NOT NULL,
                 date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 date_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 content BYTEA
             );
             {create_table} config (
                 name VARCHAR(100) NOT NULL PRIMARY KEY,
                 value JSON NOT NULL
             );
             CREATE INDEX ON releases (release_time DESC);
             CREATE INDEX content_idx ON crates USING gin(content);",
            // downgrade query
            "DROP TABLE authors, author_rels, keyword_rels, keywords, owner_rels,
                        owners, releases, crates, builds, queue, files, config;"
        ),
        migration!(
            context.clone(),
            // version
            2,
            // description
            "Added priority column to build queue",
            // upgrade query
            "ALTER TABLE queue ADD COLUMN priority INT DEFAULT 0;",
            // downgrade query
            "ALTER TABLE queue DROP COLUMN priority;"
        ),
        migration!(
            context.clone(),
            // version
            3,
            // description
            "Added sandbox_overrides table",
            // upgrade query
            "{create_table} sandbox_overrides (
                 crate_name VARCHAR NOT NULL PRIMARY KEY,
                 max_memory_bytes INTEGER,
                 timeout_seconds INTEGER
             );",
            // downgrade query
            "DROP TABLE sandbox_overrides;"
        ),
        migration!(
            context.clone(),
            4,
            "Make more fields not null",
            "ALTER TABLE releases ALTER COLUMN release_time SET NOT NULL,
                                  ALTER COLUMN yanked SET NOT NULL,
                                  ALTER COLUMN downloads SET NOT NULL",
            "ALTER TABLE releases ALTER COLUMN release_time DROP NOT NULL,
                                  ALTER COLUMN yanked DROP NOT NULL,
                                  ALTER COLUMN downloads DROP NOT NULL"
        ),
        migration!(
            context.clone(),
            // version
            5,
            // description
            "Make target_name non-nullable",
            // upgrade query
            "ALTER TABLE releases ALTER COLUMN target_name SET NOT NULL",
            // downgrade query
            "ALTER TABLE releases ALTER COLUMN target_name DROP NOT NULL",
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
