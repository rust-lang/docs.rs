use anyhow::Result;
use docs_rs_types::{CrateId, KrateName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Krate {
    pub id: CrateId,
    /// the stored name, which can differ from a requested one in `-` vs `_`
    pub name: KrateName,
}

impl Krate {
    /// Look up a crate by its normalized name.
    pub async fn find_by_name(
        conn: &mut sqlx::PgConnection,
        name: &KrateName,
    ) -> Result<Option<Self>> {
        Ok(sqlx::query_as!(
            Krate,
            r#"
            SELECT
                id as "id: CrateId",
                name as "name: KrateName"
            FROM crates
            WHERE normalize_crate_name(name) = normalize_crate_name($1)
            "#,
            name as _
        )
        .fetch_optional(&mut *conn)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{releases::initialize_crate, testing::TestDatabase};
    use docs_rs_types::testing::FOO;

    #[tokio::test(flavor = "multi_thread")]
    async fn find_by_name_uses_normalization() -> Result<()> {
        let db = TestDatabase::with_test_config().await?;
        let mut conn = db.async_conn().await?;

        let stored: KrateName = "Some_Package".parse()?;
        let id = initialize_crate(&mut conn, &stored).await?;

        assert_eq!(
            Krate::find_by_name(&mut conn, &"some-package".parse()?).await?,
            Some(Krate { id, name: stored })
        );
        assert_eq!(Krate::find_by_name(&mut conn, &FOO).await?, None);
        Ok(())
    }
}
