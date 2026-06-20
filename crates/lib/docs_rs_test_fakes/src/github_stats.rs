use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD as b64};

#[derive(bon::Builder)]
#[builder(on(_, into))]
pub struct FakeGithubStats {
    repo: String,

    #[builder(default)]
    stars: i32,

    #[builder(default)]
    forks: i32,

    #[builder(default)]
    issues: i32,
}

impl FakeGithubStats {
    pub async fn create(&self, conn: &mut sqlx::PgConnection) -> Result<i32> {
        let existing_count: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM repositories")
            .fetch_one(&mut *conn)
            .await?
            .unwrap();
        let host_id = b64.encode(format!("FAKE ID {existing_count}"));

        let id = sqlx::query_scalar!(
            "INSERT INTO repositories (host, host_id, name, description, last_commit, stars, forks, issues, updated_at)
             VALUES ('github.com', $1, $2, 'Fake description!', NOW(), $3, $4, $5, NOW())
             RETURNING id",
            host_id, self.repo, self.stars, self.forks, self.issues,
        ).fetch_one(&mut *conn).await?;

        Ok(id)
    }
}

use fake_github_stats_builder::{IsComplete, State};

impl<S: State> FakeGithubStatsBuilder<S> {
    pub async fn create(self, conn: &mut sqlx::PgConnection) -> Result<i32>
    where
        S: IsComplete,
    {
        self.build().create(conn).await
    }
}
