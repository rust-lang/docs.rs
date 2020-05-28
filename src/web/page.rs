//! Generic page struct

use super::{
    builds::Build, crate_details::CrateDetails, releases::Release, source::FileList, MetaData,
};
use crate::{docbuilder::Limits, error::Result};
use arc_swap::ArcSwap;
use iron::{headers::ContentType, response::Response, status::Status, IronResult};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use tera::{Context, Result as TeraResult, Tera};

lazy_static::lazy_static! {
    // TODO: Remove this, it's just for keeping `Page` happy
    static ref RUSTC_RESOURCE_SUFFIX: String = load_rustc_resource_suffix()
        .unwrap_or_else(|_| "???".into());


    /// Holds all data relevant to templating
    pub(crate) static ref TEMPLATE_DATA: TemplateData = TemplateData::new().expect("Failed to load template data");
}

/// Holds all data relevant to templating
pub(crate) struct TemplateData {
    /// The actual templates, stored in an `ArcSwap` so that they're hot-swappable
    // TODO: Conditional compilation so it's not always wrapped, the `ArcSwap` is unneeded overhead for prod
    templates: ArcSwap<Tera>,
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
// TODO: This can be done in a better way, right?
fn timeformat(value: &Value, args: &HashMap<String, Value>) -> TeraResult<Value> {
    let fmt = if let Some(Value::Bool(true)) = args.get("relative") {
        let value = time::strptime(value.as_str().unwrap(), "%Y-%m-%dT%H:%M:%S%z").unwrap();

        super::duration_to_str(value.to_timespec())
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlobalAlert {
    pub(crate) url: &'static str,
    pub(crate) text: &'static str,
    pub(crate) css_class: &'static str,
    pub(crate) fa_icon: &'static str,
}

pub trait WebPage: Serialize + Sized {
    /// Turn the current instance into a `Response`, ready to be served
    // TODO: The potential for caching similar pages is here due to render taking `&Context`
    fn into_response(self) -> IronResult<Response> {
        let ctx = Context::from_serialize(&self).unwrap();
        let rendered = TEMPLATE_DATA
            .templates
            .load()
            .render(Self::template(), &ctx)
            .unwrap();

        let mut response = Response::with((self.get_status(), rendered));
        response.headers.set(Self::content_type());

        Ok(response)
    }

    /// Get the name of the template to be rendered
    fn template() -> &'static str;

    /// Gets the status of the request, defaults to `Ok`
    fn get_status(&self) -> Status {
        Status::Ok
    }

    /// The content type that the template should be served with, defaults to html
    fn content_type() -> ContentType {
        ContentType::html()
    }
}

/// The sitemap, corresponds with `templates/sitemap.xml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SitemapXml {
    /// The releases to be displayed on the sitemap
    pub releases: Vec<(String, String)>,
}

impl WebPage for SitemapXml {
    fn template() -> &'static str {
        "docsrs/sitemap.xml"
    }

    fn content_type() -> ContentType {
        ContentType("application/xml".parse().unwrap())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct About {
    pub rustc_version: Option<String>,
    pub limits: Limits,
}

impl WebPage for About {
    fn template() -> &'static str {
        "docsrs/about.html"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HomePage {
    pub recent_releases: Vec<Release>,
}

impl WebPage for HomePage {
    fn template() -> &'static str {
        "docsrs/home.html"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReleaseFeed {
    pub recent_releases: Vec<Release>,
}

impl WebPage for ReleaseFeed {
    fn template() -> &'static str {
        "releases/feed.xml"
    }

    fn content_type() -> ContentType {
        ContentType("application/atom+xml".parse().unwrap())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ViewReleases {
    pub releases: Vec<Release>,
    pub description: String,
    pub release_type: ReleaseType,
    pub show_next_page: bool,
    pub show_previous_page: bool,
    pub page_number: i64,
    pub author: Option<String>,
}

impl WebPage for ViewReleases {
    fn template() -> &'static str {
        "releases/releases.html"
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReleaseType {
    Recent,
    Stars,
    RecentFailures,
    Failures,
    Author,
    Search,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ReleaseActivity {
    pub description: String,
    pub activity_data: Value,
}

impl WebPage for ReleaseActivity {
    fn template() -> &'static str {
        "releases/activity.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ReleaseQueue {
    pub description: String,
    pub queue_is_empty: bool,
    pub queue: Vec<(String, String, i32)>,
}

impl WebPage for ReleaseQueue {
    fn template() -> &'static str {
        "releases/queue.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CrateDetailsPage {
    pub details: Option<CrateDetails>,
}

impl WebPage for CrateDetailsPage {
    fn template() -> &'static str {
        "crate/details.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BuildsPage {
    pub metadata: Option<MetaData>,
    pub builds: Vec<Build>,
    pub build_log: Option<Build>,
    pub limits: Limits,
}

impl WebPage for BuildsPage {
    fn template() -> &'static str {
        "crate/builds.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct RustdocPage {
    pub latest_path: String,
    pub latest_version: String,
    pub inner_path: String,
    pub is_latest_version: bool,
    pub rustdoc_head: String,
    pub rustdoc_body: String,
    pub rustdoc_body_class: String,
    pub krate: CrateDetails,
}

impl WebPage for RustdocPage {
    fn template() -> &'static str {
        "rustdoc/page.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SourcePage {
    pub file_list: FileList,
    pub show_parent_link: bool,
    pub file_content: Option<String>,
    pub is_rust_source: bool,
}

impl WebPage for SourcePage {
    fn template() -> &'static str {
        "crate/source.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Error {
    pub title: String,
    pub search_query: Option<String>,
    #[serde(skip)]
    pub status: iron::status::Status,
}

impl WebPage for Error {
    fn template() -> &'static str {
        "error.html"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Search {
    pub title: String,
    #[serde(rename = "releases")]
    pub results: Vec<Release>,
    pub search_query: Option<String>,
    pub previous_page_button: bool,
    pub next_page_button: bool,
    pub current_page: i64,
    /// This should always be `ReleaseType::Search`
    pub release_type: ReleaseType,
    #[serde(skip)]
    pub status: iron::status::Status,
}

impl WebPage for Search {
    fn template() -> &'static str {
        "releases/releases.html"
    }
}

impl Default for Search {
    fn default() -> Self {
        Self {
            title: String::default(),
            results: Vec::default(),
            search_query: None,
            previous_page_button: false,
            next_page_button: false,
            current_page: 0,
            release_type: ReleaseType::Search,
            status: iron::status::Ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_global_alert() {
        let alert = GlobalAlert {
            url: "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            text: "THE WORLD WILL SOON END",
            css_class: "THE END IS NEAR",
            fa_icon: "https://gph.is/1uOvmqR",
        };

        let correct_json = json!({
            "url": "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            "text": "THE WORLD WILL SOON END",
            "css_class": "THE END IS NEAR",
            "fa_icon": "https://gph.is/1uOvmqR"
        });

        assert_eq!(correct_json, serde_json::to_value(&alert).unwrap());
    }

    #[test]
    fn test_templates_are_valid() {
        let tera = load_templates().unwrap();
        tera.check_macro_files().unwrap();
    }
}
