use crate::{PRIORITY_DEFAULT, PRIORITY_DEPRIORITIZED};
use anyhow::{Context as _, Result};
use docs_rs_database::Pool;
use docs_rs_repository_stats::workspaces::{
    get_crate_names_for_repository_build_priorities, get_crate_names_from_big_workspaces,
};
use docs_rs_types::{Duration, KrateName};
use futures_util::stream::TryStreamExt;
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};
use tokio::sync::Mutex;
use tracing::info;

const PRIORITY_RELOAD_FREQUENCY: Duration = Duration::from_secs(300); // 5 minutes

/// cached crate priorities.
///
/// Load & caches necessary data to figure out the wanted priority for a crate build.
#[derive(Debug)]
pub(crate) struct PrioritiesCache {
    inner: Mutex<PrioritiesCacheInner>,
}

#[derive(Debug)]
struct PrioritiesCacheInner {
    last_reload: Option<Instant>,
    pool: Pool,
    deprioritize_workspace_size: u16,
    patterns: Vec<(Regex, i32)>,
    workspace_overrides: HashMap<KrateName, i32>,
    big_workspace_crate_names: HashSet<KrateName>,
}

impl PrioritiesCacheInner {
    async fn reload(&mut self) -> Result<()> {
        let mut conn = self.pool.get_async().await?;

        let patterns = list_crate_priorities(&mut conn)
            .await?
            .into_iter()
            .map(|(pattern, priority)| -> Result<_> {
                let re = compile_like_pattern(&pattern)
                    .with_context(|| format!("can't compile pattern {} into regex", pattern))?;
                Ok((re, priority))
            })
            .collect::<Result<Vec<_>>>()?;

        let workspace_overrides =
            get_crate_names_for_repository_build_priorities(&mut conn).await?;
        let workspace_crate_names =
            get_crate_names_from_big_workspaces(&mut conn, self.deprioritize_workspace_size)
                .await?;

        self.patterns = patterns;
        self.workspace_overrides = workspace_overrides;
        self.big_workspace_crate_names = workspace_crate_names;
        self.last_reload = Some(Instant::now());

        info!(
            patterns_len = self.patterns.len(),
            workspace_overrides_len = self.workspace_overrides.len(),
            workspace_crate_names_len = self.big_workspace_crate_names.len(),
            deprioritize_workspace_size = self.deprioritize_workspace_size,
            "loaded crate priorities"
        );

        Ok(())
    }

    fn priority_from_pattern(&self, krate: &KrateName) -> Option<i32> {
        self.patterns
            .iter()
            .find_map(|(regex, prio)| regex.is_match(krate.as_str()).then_some(*prio))
    }

    fn priority_from_workspace_override(&self, krate: &KrateName) -> Option<i32> {
        self.workspace_overrides.get(krate).copied()
    }

    fn priority_from_big_workspaces(&self, krate: &KrateName) -> Option<i32> {
        self.big_workspace_crate_names
            .contains(krate)
            .then_some(PRIORITY_DEPRIORITIZED)
    }
}

impl PrioritiesCache {
    pub(crate) fn new(pool: Pool, deprioritize_workspace_size: u16) -> Self {
        let inner = PrioritiesCacheInner {
            pool,
            last_reload: None,
            deprioritize_workspace_size,
            patterns: Vec::new(),
            workspace_overrides: HashMap::new(),
            big_workspace_crate_names: HashSet::new(),
        };

        PrioritiesCache {
            inner: Mutex::new(inner),
        }
    }

    /// get the priority for a crate.
    ///
    /// Checks in order:
    /// 1. priority overrides via name pattern
    /// 2. workspace / repo priority overrides
    /// 3. big workspace deprio
    /// 4. default prio
    pub(crate) async fn get(&self, krate: &KrateName) -> Result<i32> {
        let mut inner = self.inner.lock().await;

        if inner
            .last_reload
            .is_none_or(|last_reload| last_reload.elapsed() > (*PRIORITY_RELOAD_FREQUENCY))
        {
            inner.reload().await?;
        };

        Ok(inner
            .priority_from_pattern(krate)
            .or_else(|| inner.priority_from_workspace_override(krate))
            .or_else(|| inner.priority_from_big_workspaces(krate))
            .unwrap_or(PRIORITY_DEFAULT))
    }

    /// force a reload of the cached priority data, ignoring the reload frequency.
    #[cfg(test)]
    pub(crate) async fn reload(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.reload().await
    }
}

/// compile a postgres LIKE pattern to a regex so we can match it in rust.
///
/// for now we compile the postgres pattern to regex, so we can easily revert this PR.
/// Later we can just write regexes into the table and directly use them.
fn compile_like_pattern(pattern: &str) -> Result<Regex> {
    let mut regex = String::from("^");
    let mut chars = pattern.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '%' => regex.push_str(".*"),
            '_' => regex.push('.'),

            // Postgres LIKE uses backslash as the default escape char.
            // So `\%` means literal percent, `\_` means literal underscore.
            '\\' => {
                if let Some(escaped) = chars.next() {
                    regex.push_str(&regex::escape(&escaped.to_string()));
                } else {
                    regex.push_str(&regex::escape("\\"));
                }
            }

            literal => {
                regex.push_str(&regex::escape(&literal.to_string()));
            }
        }
    }

    regex.push('$');
    Ok(Regex::new(&regex)?)
}

/// Get the build queue priority for a crate, returns the matching pattern too
pub async fn list_crate_priorities(conn: &mut sqlx::PgConnection) -> Result<Vec<(String, i32)>> {
    Ok(
        sqlx::query!("SELECT pattern, priority FROM crate_priorities")
            .fetch(conn)
            .map_ok(|r| (r.pattern, r.priority))
            .try_collect()
            .await?,
    )
}

/// Get the build queue priority for a crate with its matching pattern
pub async fn get_crate_pattern_and_priority(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
) -> Result<Option<(String, i32)>> {
    // Search the `priority` table for a priority where the crate name matches the stored pattern
    Ok(sqlx::query!(
        "SELECT pattern, priority FROM crate_priorities WHERE $1 LIKE pattern LIMIT 1",
        name as _
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|row| (row.pattern, row.priority)))
}

/// Set all crates that match [`pattern`] to have a certain priority
///
/// Note: `pattern` is used in a `LIKE` statement, so it must follow the postgres like syntax
///
/// [`pattern`]: https://www.postgresql.org/docs/8.3/functions-matching.html
pub async fn set_crate_priority(
    conn: &mut sqlx::PgConnection,
    pattern: &str,
    priority: i32,
) -> Result<()> {
    sqlx::query!(
        "INSERT INTO crate_priorities (pattern, priority) VALUES ($1, $2)",
        pattern,
        priority,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Remove a pattern from the priority table, returning the priority that it was associated with or `None`
/// if nothing was removed
pub async fn remove_crate_priority(
    conn: &mut sqlx::PgConnection,
    pattern: &str,
) -> Result<Option<i32>> {
    Ok(sqlx::query_scalar!(
        "DELETE FROM crate_priorities WHERE pattern = $1 RETURNING priority",
        pattern,
    )
    .fetch_optional(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::testing::TestDatabase;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_repository_stats::workspaces::{
        rewrite_repository_stats, set_repository_build_priority,
    };
    use docs_rs_storage::testing::TestStorage;
    use docs_rs_test_fakes::{FakeGithubStats, FakeRelease};
    use docs_rs_types::testing::{BAR, BAZ, FOO};
    use test_case::test_case;

    const PRIO: i32 = -100;
    const REPO: &str = "owner1/repo1";

    struct TestEnv {
        db: TestDatabase,
        storage: TestStorage,
    }

    impl TestEnv {
        async fn fake_release(&self) -> FakeRelease<'_> {
            FakeRelease::new(self.db.pool().clone(), self.storage.storage().clone())
        }
    }

    async fn test_env() -> Result<TestEnv> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await?;

        let storage = TestStorage::from_config(
            docs_rs_storage::Config::test_config()?.into(),
            test_metrics.provider(),
        )
        .await?;

        Ok(TestEnv { db, storage })
    }

    #[test_case("", &[""], &["a"]; "empty pattern")]
    #[test_case(
        "foo.bar+",
        &["foo.bar+"],
        &["fooXbar+", "foo.bar"];
        "regex metacharacters"
    )]
    #[test_case(
        "foo%%_%bar",
        &["foo-xbar", "foo-long-xbar"],
        &["foobar", "foo-bar-baz"];
        "mixed consecutive wildcards"
    )]
    #[test_case(
        r"literal\%percent",
        &["literal%percent"],
        &["literal-percent", "literalXXpercent"];
        "escaped percent"
    )]
    #[test_case(
        r"literal\_underscore",
        &["literal_underscore"],
        &["literal-underscore", "literalXunderscore"];
        "escaped underscore"
    )]
    #[test_case(
        r"literal\\backslash",
        &[r"literal\backslash"],
        &["literal/backslash", r"literal\\backslash"];
        "escaped backslash"
    )]
    #[test_case(
        "trailing\\",
        &["trailing\\"],
        &["trailing", "trailing/"];
        "trailing backslash"
    )]
    #[test_case(
        "cranelift-%",
        &["cranelift-asdf", "cranelift-asdf-fb"],
        &["other-xx" ];
        "prod example 1"
    )]
    #[test_case(
        r"azure\_mgmt\_%",
        &["azure_mgmt_123", "azure_mgmt_abc"],
        &["azure-mgmt-123" ];
        "prod example 2"
    )]
    #[test_case("_", &["é", "🦀"], &["", "éé"]; "unicode wildcard")]
    fn compile_like_pattern_handles_edge_cases(
        pattern: &str,
        should_match: &[&str],
        should_not_match: &[&str],
    ) -> Result<()> {
        let regex = compile_like_pattern(pattern)?;

        for value in should_match {
            assert!(regex.is_match(value), "{pattern:?} should match {value:?}");
        }

        for value in should_not_match {
            assert!(
                !regex.is_match(value),
                "{pattern:?} should not match {value:?}"
            );
        }

        Ok(())
    }

    #[test_case(
        "docsrs-%",
        &["docsrs-database", "docsrs-", "docsrs-s3", "docsrs-webserver"],
        &["docsrs"]
    )]
    #[test_case(
        "_c_",
        &["rcc"],
        &["rc"]
    )]
    #[test_case(
        "hexponent",
        &["hexponent"],
        &["hexponents", "floathexponent"]
    )]
    #[tokio::test(flavor = "multi_thread")]
    async fn set_priority(
        pattern: &str,
        should_match: &[&str],
        should_not_match: &[&str],
    ) -> Result<()> {
        let env = test_env().await?;

        let mut conn = env.db.async_conn().await?;

        set_crate_priority(&mut conn, pattern, PRIO).await?;

        let priorities = PrioritiesCache::new(env.db.pool().clone(), 20);

        for name in should_match {
            let krate: KrateName = name.parse().unwrap();
            assert_eq!(priorities.get(&krate).await?, PRIO);
        }

        for name in should_not_match {
            let krate: KrateName = name.parse().unwrap();
            assert_eq!(priorities.get(&krate).await?, PRIORITY_DEFAULT);
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_priority() -> Result<()> {
        let env = test_env().await?;

        let mut conn = env.db.async_conn().await?;
        let pattern = "docsrs-%";
        let krate = KrateName::from_static("docsrs-");

        set_crate_priority(&mut conn, pattern, PRIO).await?;
        let priorities = PrioritiesCache::new(env.db.pool().clone(), 20);
        assert_eq!(priorities.get(&krate).await?, PRIO);

        assert_eq!(remove_crate_priority(&mut conn, pattern).await?, Some(PRIO));
        priorities.reload().await?;
        assert_eq!(priorities.get(&krate).await?, PRIORITY_DEFAULT);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_default_priority() -> Result<()> {
        let env = test_env().await?;

        let priorities = PrioritiesCache::new(env.db.pool().clone(), 20);

        for name in &["docsrs", "rcc", "lasso", "hexponent", "rust4lyfe"] {
            let krate = KrateName::from_static(name);

            assert_eq!(priorities.get(&krate).await?, PRIORITY_DEFAULT);
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn override_workspace_priority() -> Result<()> {
        let env = test_env().await?;

        let mut conn = env.db.async_conn().await?;

        env.fake_release()
            .await
            .name(&FOO)
            .github_stats(REPO, 0, 0, 0)
            .create()
            .await?;

        set_repository_build_priority(&mut conn, REPO, -5).await?;

        assert_eq!(
            get_crate_names_for_repository_build_priorities(&mut conn,).await?,
            HashMap::from_iter([(FOO, -5)])
        );

        let priorities = PrioritiesCache::new(env.db.pool().clone(), 20);

        // repo override is used
        assert_eq!(priorities.get(&FOO).await?, -5);

        // no override, default prio
        assert_eq!(priorities.get(&BAR).await?, PRIORITY_DEFAULT);

        // set pattern priority, should be used instead of the repo prio
        set_crate_priority(&mut conn, FOO.as_ref(), PRIO).await?;

        priorities.reload().await?;
        assert_eq!(priorities.get(&FOO).await?, PRIO);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_auto_deprio_workspace() -> Result<()> {
        let env = test_env().await?;

        let mut conn = env.db.async_conn().await?;

        let prioritized_repo_id = FakeGithubStats::builder()
            .repo(REPO)
            .create(&mut conn)
            .await?;

        // two crates, one repo
        for name in [FOO, BAR] {
            env.fake_release()
                .await
                .name(&name)
                .github_stats_id(prioritized_repo_id)
                .create()
                .await?;
        }

        rewrite_repository_stats(&mut conn).await?;

        // validate our helper method,
        // TODO: could move to stats/workspaces module
        assert!(
            get_crate_names_from_big_workspaces(&mut conn, 3)
                .await?
                .is_empty()
        );
        assert_eq!(
            get_crate_names_from_big_workspaces(&mut conn, 2).await?,
            HashSet::from_iter([FOO, BAR])
        );

        let priorities = PrioritiesCache::new(env.db.pool().clone(), 2);

        // workspace size override is used
        assert_eq!(priorities.get(&FOO).await?, PRIORITY_DEPRIORITIZED);
        assert_eq!(priorities.get(&BAR).await?, PRIORITY_DEPRIORITIZED);
        assert_eq!(priorities.get(&BAZ).await?, PRIORITY_DEFAULT);

        // set pattern priority, should be used instead of the workspace size prio
        set_crate_priority(&mut conn, FOO.as_ref(), PRIO).await?;

        priorities.reload().await?;
        assert_eq!(priorities.get(&FOO).await?, PRIO);

        Ok(())
    }
}
