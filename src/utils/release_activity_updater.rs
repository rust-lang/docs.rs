use crate::db::connect_db;
use crate::error::Result;
use serde_json::{Map, Value};
use time::{now, Duration};

pub fn update_release_activity() -> Result<()> {
    let conn = connect_db()?;
    let mut dates = Vec::with_capacity(30);
    let mut crate_counts = Vec::with_capacity(30);
    let mut failure_counts = Vec::with_capacity(30);

    for day in 0..30 {
        let rows = conn.query(
            &format!(
                "SELECT COUNT(*)
                 FROM releases
                 WHERE release_time < NOW() - INTERVAL '{} day' AND
                       release_time > NOW() - INTERVAL '{} day'",
                day,
                day + 1
            ),
            &[],
        )?;
        let failures_count_rows = conn.query(
            &format!(
                "SELECT COUNT(*)
                 FROM releases
                 WHERE is_library = TRUE AND
                       build_status = FALSE AND
                       release_time < NOW() - INTERVAL '{} day' AND
                       release_time > NOW() - INTERVAL '{} day'",
                day,
                day + 1
            ),
            &[],
        )?;

        let release_count: i64 = rows.get(0).get(0);
        let failure_count: i64 = failures_count_rows.get(0).get(0);
        let now = now();
        let date = now - Duration::days(day);

        // unwrap is fine here, as our date format is always valid
        dates.push(format!("{}", date.strftime("%d %b").unwrap()));
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

#[cfg(test)]
mod test {
    use super::update_release_activity;

    #[test]
    #[ignore]
    fn test_update_release_activity() {
        crate::test::init_logger();
        assert!(update_release_activity().is_ok());
    }
}
