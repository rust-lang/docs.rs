use super::TEMPLATE_DATA;
use crate::{
    docbuilder::Limits,
    web::{
        builds::Build, crate_details::CrateDetails, releases::Release, source::FileList, MetaData,
    },
};
use iron::{headers::ContentType, response::Response, status::Status, IronResult};
use serde::Serialize;
use serde_json::Value;
use tera::Context;

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
