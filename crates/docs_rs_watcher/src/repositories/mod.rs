pub use self::github::GitHub;
pub use self::gitlab::GitLab;
pub use self::updater::RepositoryStatsUpdater;

#[derive(Debug, thiserror::Error)]
#[error("rate limit reached")]
struct RateLimitReached;

mod github;
mod gitlab;
mod updater;
