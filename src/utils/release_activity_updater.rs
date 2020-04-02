
use crate::db::connect_db;
use time::{now, Duration};
use std::collections::BTreeMap;
use rustc_serialize::json::ToJson;
use crate::error::Result;


pub fn update_release_activity() -> Result<()> {

    let conn = connect_db()?;
    let mut dates = Vec::new();
    let mut crate_counts = Vec::new();
    let mut failure_counts = Vec::new();

    for day in 0..30 {
        let rows = conn.query(&format!("SELECT COUNT(*)
                                             FROM releases
                                             WHERE release_time < NOW() - INTERVAL '{} day' AND
                                                   release_time > NOW() - INTERVAL '{} day'",
                                            day,
                                            day + 1),
                                   &[])?;
        let failures_count_rows = conn.query(
                                   &format!("SELECT COUNT(*)
                                             FROM releases
                                             WHERE is_library = TRUE AND
                                                   build_status = FALSE AND
                                                   release_time < NOW() - INTERVAL '{} day' AND
                                                   release_time > NOW() - INTERVAL '{} day'",
                                            day,
                                            day + 1),
                                   &[])?;
        let release_count: i64 = rows.get(0).get(0);
        let failure_count: i64 = failures_count_rows.get(0).get(0);
        let now = now();
        let date = now - Duration::days(day);
        dates.push(format!("{}", date.strftime("%d %b").unwrap()));
        // unwrap is fine here,             ~~~~~~~~~~~~^  our date format is always valid
        crate_counts.push(release_count);
        failure_counts.push(failure_count);
    }

    dates.reverse();
    crate_counts.reverse();
    failure_counts.reverse();

    let map = {
        let mut map = BTreeMap::new();
        map.insert("dates".to_owned(), dates.to_json());
        map.insert("counts".to_owned(), crate_counts.to_json());
        map.insert("failures".to_owned(), failure_counts.to_json());
        map.to_json()
    };

    conn.query("INSERT INTO config (name, value) VALUES ('release_activity', $1)",
               &[&map])
        .or_else(|_| {
            conn.query("UPDATE config SET value = $1 WHERE name = 'release_activity'",
                       &[&map])
        })?;

    Ok(())
}


#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::update_release_activity;

    #[test]
    #[ignore]
    fn test_update_release_activity() {
        let _ = env_logger::try_init();
        assert!(update_release_activity().is_ok());
    }
}
