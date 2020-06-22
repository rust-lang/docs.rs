//! Generic page struct

use handlebars_iron::Template;
use iron::response::Response;
use iron::{status, IronResult, Set};
use serde::{
    ser::{SerializeStruct, Serializer},
    Serialize,
};
use serde_json::Value;
use std::collections::BTreeMap;

lazy_static::lazy_static! {
    static ref RUSTC_RESOURCE_SUFFIX: String = load_rustc_resource_suffix()
        .unwrap_or_else(|_| "???".into());
}

fn load_rustc_resource_suffix() -> Result<String, failure::Error> {
    // New instances of the configuration or the connection pool shouldn't be created inside the
    // application, but we're removing handlebars so this is not going to be a problem in the long
    // term. To avoid wasting resources, the pool is hardcoded to only keep one connection alive.
    let mut config = crate::Config::from_env()?;
    config.max_pool_size = 1;
    config.min_pool_idle = 1;
    let pool = crate::db::Pool::new(&config)?;
    let conn = pool.get()?;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T: Serialize> {
    title: Option<String>,
    content: T,
    status: status::Status,
    varss: BTreeMap<String, String>,
    varsb: BTreeMap<String, bool>,
    varsi: BTreeMap<String, i64>,
    rustc_resource_suffix: &'static str,
}

impl<T: Serialize> Page<T> {
    pub fn new(content: T) -> Page<T> {
        Page {
            title: None,
            content,
            status: status::Ok,
            varss: BTreeMap::new(),
            varsb: BTreeMap::new(),
            varsi: BTreeMap::new(),
            rustc_resource_suffix: &RUSTC_RESOURCE_SUFFIX,
        }
    }

    /// Sets a string variable
    pub fn set(mut self, var: &str, val: &str) -> Page<T> {
        self.varss.insert(var.to_owned(), val.to_owned());
        self
    }

    /// Sets a boolean variable
    pub fn set_bool(mut self, var: &str, val: bool) -> Page<T> {
        self.varsb.insert(var.to_owned(), val);
        self
    }

    /// Sets a boolean variable to true
    pub fn set_true(mut self, var: &str) -> Page<T> {
        self.varsb.insert(var.to_owned(), true);
        self
    }

    /// Sets title of page
    pub fn title(mut self, title: &str) -> Page<T> {
        self.title = Some(title.to_owned());
        self
    }

    /// Sets status code for response
    pub fn set_status(mut self, s: status::Status) -> Page<T> {
        self.status = s;
        self
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_resp(self, template: &str) -> IronResult<Response> {
        let mut resp = Response::new();
        let status = self.status;
        let temp = Template::new(template, self);
        resp.set_mut(temp).set_mut(status);

        Ok(resp)
    }
}

impl<T: Serialize> Serialize for Page<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Make sure that the length parameter passed to serde is correct by
        // adding the someness of the global alert to the total. `true`
        // is 1 and `false` is 0, so it increments if the value is some (and therefore
        // needs to be serialized)
        let mut state = serializer.serialize_struct(
            "Page",
            8 + crate::GLOBAL_ALERT.is_some() as usize + self.title.is_some() as usize,
        )?;

        if let Some(ref title) = self.title {
            state.serialize_field("title", title)?;
        }

        state.serialize_field("has_global_alert", &crate::GLOBAL_ALERT.is_some())?;
        if let Some(ref global_alert) = crate::GLOBAL_ALERT {
            state.serialize_field("global_alert", global_alert)?;
        }

        state.serialize_field("content", &self.content)?;
        state.serialize_field("rustc_resource_suffix", self.rustc_resource_suffix)?;
        state.serialize_field("cratesfyi_version", crate::BUILD_VERSION)?;
        state.serialize_field(
            "cratesfyi_version_safe",
            &build_version_safe(crate::BUILD_VERSION),
        )?;
        state.serialize_field("varss", &self.varss)?;
        state.serialize_field("varsb", &self.varsb)?;
        state.serialize_field("varsi", &self.varsi)?;

        state.end()
    }
}

fn build_version_safe(version: &str) -> String {
    version.replace(" ", "-").replace("(", "").replace(")", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::releases::{self, Release};
    use chrono::Utc;
    use iron::Url;
    use serde_json::json;

    #[test]
    fn load_page_from_releases() {
        crate::test::wrapper(|env| {
            let db = env.db();
            db.fake_release().name("foo").version("0.1.0").create()?;
            let packages = releases::get_releases(&db.conn(), 1, 1, releases::Order::ReleaseTime);

            let mut varsb = BTreeMap::new();
            varsb.insert("show_search_form".into(), true);
            varsb.insert("hide_package_navigation".into(), true);

            let correct_page = Page {
                title: None,
                content: packages.clone(),
                status: status::Status::Ok,
                varss: BTreeMap::new(),
                varsb,
                varsi: BTreeMap::new(),
                rustc_resource_suffix: &RUSTC_RESOURCE_SUFFIX,
            };

            let page = Page::new(packages)
                .set_true("show_search_form")
                .set_true("hide_package_navigation");

            assert_eq!(page, correct_page);

            Ok(())
        })
    }

    #[test]
    fn build_version_url_safe() {
        let safe = format!(
            "https://docs.rs/builds/{}",
            build_version_safe(crate::BUILD_VERSION)
        );
        assert!(Url::parse(&safe).is_ok());
    }
}
