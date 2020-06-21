use crate::db::Pool;
use crate::error::Result;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use notify::{watcher, RecursiveMode, Watcher};
use postgres::Connection;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{mpsc::channel, Arc};
use std::thread;
use std::time::Duration;
use tera::{Result as TeraResult, Tera};

/// Holds all data relevant to templating
#[derive(Debug)]
pub(crate) struct TemplateData {
    /// The actual templates, stored in an `ArcSwap` so that they're hot-swappable
    // TODO: Conditional compilation so it's not always wrapped, the `ArcSwap` is unneeded overhead for prod
    pub templates: ArcSwap<Tera>,
}

impl TemplateData {
    pub(crate) fn new(conn: &Connection) -> Result<Self> {
        log::trace!("Loading templates");

        let data = Self {
            templates: ArcSwap::from_pointee(load_templates(conn)?),
        };

        log::trace!("Finished loading templates");

        Ok(data)
    }

    pub(crate) fn start_template_reloading(template_data: Arc<TemplateData>, pool: Pool) {
        let (tx, rx) = channel();
        // Set a 2 second event debounce for the watcher
        let mut watcher = watcher(tx, Duration::from_secs(2)).unwrap();

        watcher
            .watch("tera-templates", RecursiveMode::Recursive)
            .unwrap();

        thread::spawn(move || {
            fn reload(template_data: &TemplateData, pool: &Pool) -> Result<()> {
                let conn = pool.get()?;
                template_data
                    .templates
                    .swap(Arc::new(load_templates(&conn)?));
                log::info!("Reloaded templates");

                Ok(())
            }

            // The watcher needs to be moved into the thread so that it's not dropped (when dropped,
            // all updates cease)
            let _watcher = watcher;

            while rx.recv().is_ok() {
                if let Err(err) = reload(&template_data, &pool) {
                    log::error!("failed to reload templates: {:?}", err);
                }
            }
        });
    }
}

fn load_rustc_resource_suffix(conn: &Connection) -> Result<String> {
    let res = conn.query(
        "SELECT value FROM config WHERE name = 'rustc_version';",
        &[],
    )?;
    if res.is_empty() {
        failure::bail!("missing rustc version");
    }

    if let Some(Ok(vers)) = res.get(0).get_opt::<_, Value>("value") {
        if let Some(vers_str) = vers.as_str() {
            return Ok(crate::utils::parse_rustc_version(vers_str)?);
        }
    }

    failure::bail!("failed to parse the rustc version");
}

pub(super) fn load_templates(conn: &Connection) -> Result<Tera> {
    let mut tera = Tera::new("tera-templates/**/*")?;

    // This function will return any global alert, if present.
    ReturnValue::add_function_to(
        &mut tera,
        "global_alert",
        serde_json::to_value(crate::GLOBAL_ALERT)?,
    );
    // This function will return the current version of docs.rs.
    ReturnValue::add_function_to(
        &mut tera,
        "docsrs_version",
        Value::String(crate::BUILD_VERSION.into()),
    );
    // This function will return the resource suffix of the latest nightly used to build
    // documentation on docs.rs, or ??? if no resource suffix was found.
    ReturnValue::add_function_to(
        &mut tera,
        "rustc_resource_suffix",
        Value::String(load_rustc_resource_suffix(conn).unwrap_or_else(|err| {
            log::error!("Failed to load rustc resource suffix: {:?}", err);
            // This is not fatal because the server might be started before essential files are
            // generated during development. Returning "???" provides a degraded UX, but allows the
            // server to start every time.
            String::from("???")
        })),
    );

    // Custom filters
    tera.register_filter("timeformat", timeformat);
    tera.register_filter("dbg", dbg);
    tera.register_filter("dedent", dedent);

    Ok(tera)
}

/// Simple function that returns the pre-defined value.
struct ReturnValue {
    name: &'static str,
    value: Value,
}

impl ReturnValue {
    fn add_function_to(tera: &mut Tera, name: &'static str, value: Value) {
        tera.register_function(name, Self { name, value })
    }
}

impl tera::Function for ReturnValue {
    fn call(&self, args: &HashMap<String, Value>) -> TeraResult<Value> {
        debug_assert!(args.is_empty(), format!("{} takes no args", self.name));
        Ok(self.value.clone())
    }
}

/// Prettily format a timestamp
// TODO: This can be replaced by chrono
fn timeformat(value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
    let fmt = if let Some(Value::Bool(true)) = args.get("relative") {
        let value = DateTime::parse_from_str(value.as_str().unwrap(), "%Y-%m-%dT%H:%M:%S%z")
            .unwrap()
            .with_timezone(&Utc);

        super::super::duration_to_str(value)
    } else {
        const TIMES: &[&str] = &["seconds", "minutes", "hours"];

        let mut value = value.as_f64().unwrap();
        let mut chosen_time = &TIMES[0];

        for time in &TIMES[1..] {
            if value / 60.0 >= 1.0 {
                chosen_time = time;
                value /= 60.0;
            } else {
                break;
            }
        }

        // TODO: This formatting section can be optimized, two string allocations aren't needed
        let mut value = format!("{:.1}", value);
        if value.ends_with(".0") {
            value.truncate(value.len() - 2);
        }

        format!("{} {}", value, chosen_time)
    };

    Ok(Value::String(fmt))
}

/// Print a tera value to stdout
fn dbg(value: &Value, _args: &HashMap<String, Value>) -> TeraResult<Value> {
    println!("{:?}", value);

    Ok(value.clone())
}

/// Dedent a string by removing all leading whitespace
fn dedent(value: &Value, _args: &HashMap<String, Value>) -> TeraResult<Value> {
    let string = value.as_str().expect("dedent takes a string");

    Ok(Value::String(
        string
            .lines()
            .map(|l| l.trim_start())
            .collect::<Vec<&str>>()
            .join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_templates_are_valid() {
        crate::test::wrapper(|env| {
            let db = env.db();

            let tera = load_templates(&db.conn()).unwrap();
            tera.check_macro_files().unwrap();

            Ok(())
        });
    }
}
