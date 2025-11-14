pub use self::github::GitHub;
pub use self::gitlab::GitLab;
pub(crate) use self::updater::RepositoryName;
pub use self::updater::{
    FetchRepositoriesResult, Repository, RepositoryForge, RepositoryStatsUpdater,
};

#[derive(Debug, thiserror::Error)]
#[error("rate limit reached")]
struct RateLimitReached;

mod github;
mod gitlab;
mod updater;
