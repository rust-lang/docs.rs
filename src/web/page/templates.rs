use crate::{db::Pool, error::Result};
use anyhow::Context;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use notify::{watcher, RecursiveMode, Watcher};
use path_slash::PathExt;
use postgres::Client;
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt,
    path::PathBuf,
    sync::{mpsc::channel, Arc},
    thread,
    time::Duration,
};
use tera::{Result as TeraResult, Tera};
use walkdir::WalkDir;

const TEMPLATES_DIRECTORY: &str = "templates";

/// Holds all data relevant to templating
#[derive(Debug)]
pub(crate) struct TemplateData {
    /// The actual templates, stored in an `ArcSwap` so that they're hot-swappable
    // TODO: Conditional compilation so it's not always wrapped, the `ArcSwap` is unneeded overhead for prod
    pub templates: ArcSwap<Tera>,
}

impl TemplateData {
    pub(crate) fn new(conn: &mut Client) -> Result<Self> {
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
            .watch(TEMPLATES_DIRECTORY, RecursiveMode::Recursive)
            .unwrap();

        thread::spawn(move || {
            fn reload(template_data: &TemplateData, pool: &Pool) -> Result<()> {
                let mut conn = pool.get()?;
                template_data
                    .templates
                    .swap(Arc::new(load_templates(&mut conn)?));

                Ok(())
            }

            // The watcher needs to be moved into the thread so that it's not dropped (when dropped,
            // all updates cease)
            let _watcher = watcher;

            while rx.recv().is_ok() {
                if let Err(err) = reload(&template_data, &pool) {
                    log::error!("failed to reload templates: {}", err);
                } else {
                    log::info!("reloaded templates");
                }
            }
        });
    }
}

fn load_rustc_resource_suffix(conn: &mut Client) -> Result<String> {
    let res = conn.query(
        "SELECT value FROM config WHERE name = 'rustc_version';",
        &[],
    )?;

    if res.is_empty() {
        anyhow::bail!("missing rustc version");
    }

    if let Ok(vers) = res[0].try_get::<_, Value>("value") {
        if let Some(vers_str) = vers.as_str() {
            return crate::utils::parse_rustc_version(vers_str);
        }
    }

    anyhow::bail!("failed to parse the rustc version");
}

pub(super) fn load_templates(conn: &mut Client) -> Result<Tera> {
    // This uses a custom function to find the templates in the filesystem instead of Tera's
    // builtin way (passing a glob expression to Tera::new), speeding up the startup of the
    // application and running the tests.
    //
    // The problem with Tera's template loading code is, it walks all the files in the current
    // directory and matches them against the provided glob expression. Unfortunately this means
    // Tera will walk all the rustwide workspaces, the git repository and a bunch of other
    // unrelated data, slowing down the search a lot.
    //
    // TODO: remove this when https://github.com/Gilnaa/globwalk/issues/29 is fixed
    let mut tera = Tera::default();
    let template_files = find_templates_in_filesystem(TEMPLATES_DIRECTORY).with_context(|| {
        format!(
            "failed to search {:?} for tera templates",
            TEMPLATES_DIRECTORY
        )
    })?;
    tera.add_template_files(template_files).with_context(|| {
        format!(
            "failed while loading tera templates in {:?}",
            TEMPLATES_DIRECTORY
        )
    })?;

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
    tera.register_filter("fas", IconType::Strong);
    tera.register_filter("far", IconType::Regular);
    tera.register_filter("fab", IconType::Brand);

    Ok(tera)
}

fn find_templates_in_filesystem(base: &str) -> Result<Vec<(PathBuf, Option<String>)>> {
    let root = std::fs::canonicalize(base)?;

    let mut files = Vec::new();
    for entry in WalkDir::new(&root) {
        let entry = entry?;
        let path = entry.path();

        if !entry.metadata()?.is_file() {
            continue;
        }

        // Strip the root directory from the path and use it as the template name.
        let name = path
            .strip_prefix(&root)
            .with_context(|| format!("{} is not a child of {}", path.display(), root.display()))?
            .to_slash()
            .with_context(|| anyhow::anyhow!("failed to normalize {}", path.display()))?;
        files.push((path.to_path_buf(), Some(name)));
    }

    Ok(files)
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
        debug_assert!(args.is_empty(), "{} takes no args", self.name);
        Ok(self.value.clone())
    }
}

/// Prettily format a timestamp
// TODO: This can be replaced by chrono
#[allow(clippy::unnecessary_wraps)]
fn timeformat(value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
    let fmt = if let Some(Value::Bool(true)) = args.get("relative") {
        let value = DateTime::parse_from_rfc3339(value.as_str().unwrap())
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
#[allow(clippy::unnecessary_wraps)]
fn dbg(value: &Value, _args: &HashMap<String, Value>) -> TeraResult<Value> {
    println!("{:?}", value);

    Ok(value.clone())
}

/// Dedent a string by removing all leading whitespace
#[allow(clippy::unnecessary_wraps)]
fn dedent(value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
    let string = value.as_str().expect("dedent takes a string");

    let unindented = if let Some(levels) = args
        .get("levels")
        .map(|l| l.as_i64().expect("`levels` must be an integer"))
    {
        string
            .lines()
            .map(|mut line| {
                for _ in 0..levels {
                    // `.strip_prefix` returns `Some(suffix without prefix)` if it's successful. If it fails
                    // to strip the prefix (meaning there's less than `levels` levels of indentation),
                    // we can just quit early
                    if let Some(suffix) = line.strip_prefix("    ") {
                        line = suffix;
                    } else {
                        break;
                    }
                }

                line
            })
            .collect::<Vec<&str>>()
            .join("\n")
    } else {
        string
            .lines()
            .map(|l| l.trim_start())
            .collect::<Vec<&str>>()
            .join("\n")
    };

    Ok(Value::String(unindented))
}

enum IconType {
    Strong,
    Regular,
    Brand,
}

impl fmt::Display for IconType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let icon = match self {
            Self::Strong => "solid",
            Self::Regular => "regular",
            Self::Brand => "brands",
        };

        f.write_str(icon)
    }
}

impl tera::Filter for IconType {
    fn filter(&self, value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
        let mut aria_hidden = true;
        let icon_name = tera::escape_html(value.as_str().expect("Icons only take strings"));
        let class = if args.get("fw").and_then(|fw| fw.as_bool()).unwrap_or(false) {
            "fa-svg fa-svg-fw"
        } else {
            "fa-svg"
        };
        let aria_label = args
            .get("aria-label")
            .and_then(|l| l.as_str())
            .map(|label| {
                aria_hidden = false;
                format!(r#" aria_label="{}""#, tera::escape_html(label))
            })
            .unwrap_or_default();
        let id = args
            .get("id")
            .and_then(|l| l.as_str())
            .map(|id| format!(r#" id="{}""#, tera::escape_html(id)))
            .unwrap_or_default();
        aria_hidden = args
            .get("aria-hidden")
            .and_then(|l| l.as_bool())
            .unwrap_or(aria_hidden);

        let type_ = match self {
            IconType::Strong => font_awesome_as_a_crate::Type::Solid,
            IconType::Regular => font_awesome_as_a_crate::Type::Regular,
            IconType::Brand => font_awesome_as_a_crate::Type::Brands,
        };

        let icon_file_string = font_awesome_as_a_crate::svg(type_, &icon_name[..]).unwrap_or("");

        let icon = format!(
            "<span \
                class=\"{class} {class_extra}\" \
                aria-hidden=\"{aria_hidden}\"{aria_label}{id}>{icon_file_string}</span>",
            icon_file_string = icon_file_string,
            class = class,
            class_extra = args.get("extra").and_then(|s| s.as_str()).unwrap_or(""),
            aria_hidden = aria_hidden,
            aria_label = aria_label,
            id = id,
        );

        Ok(Value::String(icon))
    }

    fn is_safe(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_templates_are_valid() {
        crate::test::wrapper(|env| {
            let db = env.db();

            let tera = load_templates(&mut db.conn()).unwrap();
            tera.check_macro_files().unwrap();

            Ok(())
        });
    }
}
