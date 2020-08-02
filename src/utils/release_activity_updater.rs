use crate::error::Result;
use chrono::{Duration, Utc};
use postgres::Client;
use serde_json::{Map, Value};

pub fn update_release_activity(conn: &mut Client) -> Result<()> {
    let mut dates = Vec::with_capacity(30);
    let mut crate_counts = Vec::with_capacity(30);
    let mut failure_counts = Vec::with_capacity(30);

    for day in 0..30 {
        let rows = conn.query(
            format!(
                "SELECT COUNT(*)
                 FROM releases
                 WHERE release_time < NOW() - INTERVAL '{} day' AND
                       release_time > NOW() - INTERVAL '{} day'",
                day,
                day + 1
            )
            .as_str(),
            &[],
        )?;
        let failures_count_rows = conn.query(
            format!(
                "SELECT COUNT(*)
                 FROM releases
                 WHERE is_library = TRUE AND
                       build_status = FALSE AND
                       release_time < NOW() - INTERVAL '{} day' AND
                       release_time > NOW() - INTERVAL '{} day'",
                day,
                day + 1
            )
            .as_str(),
            &[],
        )?;

        let release_count: i64 = rows[0].get(0);
        let failure_count: i64 = failures_count_rows[0].get(0);
        let now = Utc::now().naive_utc();
        let date = now - Duration::days(day);

        dates.push(format!("{}", date.format("%d %b")));
        crate_counts.push(release_count);
        failure_counts.push(failure_count);
    }

    dates.reverse();
    crate_counts.reverse();
    failure_counts.reverse();

    let map = {
        let mut map = Map::new();
        map.insert("dates".to_owned(), serde_json::to_value(dates)?);
        map.insert("counts".to_owned(), serde_json::to_value(crate_counts)?);
        map.insert("failures".to_owned(), serde_json::to_value(failure_counts)?);

        Value::Object(map)
    };

    conn.query(
        "INSERT INTO config (name, value) VALUES ('release_activity', $1)
         ON CONFLICT (name) DO UPDATE
            SET value = $1 WHERE config.name = 'release_activity'",
        &[&map],
    )?;

    Ok(())
}
