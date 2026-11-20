//! Entry points for benches, which only see the public API.

use docs_rs_types::{KrateName, ReqVersion, Version};

pub async fn match_version(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
    version: &ReqVersion,
) -> anyhow::Result<Version> {
    Ok(crate::match_release::match_version(conn, name, version)
        .await
        .map_err(|err| anyhow::anyhow!("{err:?}"))?
        .into_version())
}
