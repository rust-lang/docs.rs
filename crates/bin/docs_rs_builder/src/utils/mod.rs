//! Various utilities for docs.rs

pub(crate) mod copy;
pub mod queue_builder;

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::test::async_wrapper;
//     use serde_json::Value;
//     use test_case::test_case;

//     #[test_case(ConfigName::RustcVersion, "rustc_version")]
//     #[test_case(ConfigName::QueueLocked, "queue_locked")]
//     #[test_case(ConfigName::LastSeenIndexReference, "last_seen_index_reference")]
//     fn test_configname_variants(variant: ConfigName, expected: &'static str) {
//         let name: &'static str = variant.into();
//         assert_eq!(name, expected);
//     }

//     #[test]
//     fn test_get_config_empty() {
//         async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;
//             sqlx::query!("DELETE FROM config")
//                 .execute(&mut *conn)
//                 .await?;

//             assert!(
//                 get_config::<String>(&mut conn, ConfigName::RustcVersion)
//                     .await?
//                     .is_none()
//             );
//             Ok(())
//         });
//     }

//     #[test]
//     fn test_set_and_get_config_() {
//         async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;
//             sqlx::query!("DELETE FROM config")
//                 .execute(&mut *conn)
//                 .await?;

//             assert!(
//                 get_config::<String>(&mut conn, ConfigName::RustcVersion)
//                     .await?
//                     .is_none()
//             );

//             set_config(
//                 &mut conn,
//                 ConfigName::RustcVersion,
//                 Value::String("some value".into()),
//             )
//             .await?;
//             assert_eq!(
//                 get_config(&mut conn, ConfigName::RustcVersion).await?,
//                 Some("some value".to_string())
//             );
//             Ok(())
//         });
//     }
// }
