use crate::error::Result;
use postgres::Client;

pub fn update_release_activity(conn: &mut Client) -> Result<()> {
    conn.execute("REFRESH MATERIALIZED VIEW releases_statistics", &[])?;

    Ok(())
}
