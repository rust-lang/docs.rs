use super::TEMPLATE_DATA;
use crate::error::Result;
use arc_swap::ArcSwap;
use serde_json::Value;
use std::collections::HashMap;
use tera::{Result as TeraResult, Tera};

/// Holds all data relevant to templating
pub(crate) struct TemplateData {
    /// The actual templates, stored in an `ArcSwap` so that they're hot-swappable
    // TODO: Conditional compilation so it's not always wrapped, the `ArcSwap` is unneeded overhead for prod
    pub templates: ArcSwap<Tera>,
    /// The current global alert, serialized into a json value
    global_alert: Value,
    /// The version of docs.rs, serialized into a json value
    docsrs_version: Value,
    /// The current resource suffix of rustc, serialized into a json value
    resource_suffix: Value,
}

impl TemplateData {
    pub fn new() -> Result<Self> {
        log::trace!("Loading templates");

        let data = Self {
            templates: ArcSwap::from_pointee(load_templates()?),
            global_alert: serde_json::to_value(crate::GLOBAL_ALERT)?,
            docsrs_version: Value::String(crate::BUILD_VERSION.to_owned()),
            resource_suffix: Value::String(load_rustc_resource_suffix().unwrap_or_else(|err| {
                log::error!("Failed to load rustc resource suffix: {:?}", err);
                String::from("???")
            })),
        };

        log::trace!("Finished loading templates");

        Ok(data)
    }

    pub fn start_template_reloading() {
        use std::{sync::Arc, thread, time::Duration};

        thread::spawn(|| loop {
            match load_templates() {
                Ok(templates) => {
                    log::info!("Reloaded templates");
                    TEMPLATE_DATA.templates.swap(Arc::new(templates));
                    thread::sleep(Duration::from_secs(10));
                }

                Err(err) => {
                    log::info!("Error Loading Templates:\n{}", err);
                    thread::sleep(Duration::from_secs(5));
                }
            }
        });
    }

    /// Used to initialize a `TemplateData` instance in a `lazy_static`.
    /// Loading tera takes a second, so it's important that this is done before any
    /// requests start coming in
    pub fn poke(&self) -> Result<()> {
        Ok(())
    }
}

// TODO: Is there a reason this isn't fatal? If the rustc version is incorrect (Or "???" as used by default), then
//       all pages will be served *really* weird because they'll lack all CSS
fn load_rustc_resource_suffix() -> Result<String> {
    let conn = crate::db::connect_db()?;

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

pub(super) fn load_templates() -> TeraResult<Tera> {
    let mut tera = Tera::new("templates/**/*")?;

    // Custom functions
    tera.register_function("global_alert", global_alert);
    tera.register_function("docsrs_version", docsrs_version);
    tera.register_function("rustc_resource_suffix", rustc_resource_suffix);

    // Custom filters
    tera.register_filter("timeformat", timeformat);
    tera.register_filter("dbg", dbg);
    tera.register_filter("dedent", dedent);

    Ok(tera)
}

/// Returns an `Option<GlobalAlert>` in json form for templates
fn global_alert(args: &HashMap<String, Value>) -> TeraResult<Value> {
    debug_assert!(args.is_empty(), "global_alert takes no args");

    Ok(TEMPLATE_DATA.global_alert.clone())
}

/// Returns the version of docs.rs, takes the `safe` parameter which can be `true` to get a url-safe version
fn docsrs_version(args: &HashMap<String, Value>) -> TeraResult<Value> {
    debug_assert!(
        args.is_empty(),
        "docsrs_version only takes no args, to get a safe version use `docsrs_version() | slugify`",
    );

    Ok(TEMPLATE_DATA.docsrs_version.clone())
}

/// Returns the current rustc resource suffix
fn rustc_resource_suffix(args: &HashMap<String, Value>) -> TeraResult<Value> {
    debug_assert!(args.is_empty(), "rustc_resource_suffix takes no args");

    Ok(TEMPLATE_DATA.resource_suffix.clone())
}

/// Prettily format a timestamp
// TODO: This can be replaced by chrono
fn timeformat(value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
    let fmt = if let Some(Value::Bool(true)) = args.get("relative") {
        let value = time::strptime(value.as_str().unwrap(), "%Y-%m-%dT%H:%M:%S%z").unwrap();

        super::super::duration_to_str(value.to_timespec())
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
    // TODO: This will fail as long as there are `.hbs` files in `templates/`
    // #[test]
    // fn test_templates_are_valid() {
    //     let tera = load_templates().unwrap();
    //     tera.check_macro_files().unwrap();
    // }
}
