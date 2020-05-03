//! Generic page struct

use handlebars_iron::Template;
use iron::response::Response;
use iron::{status, IronResult, Set};
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

lazy_static::lazy_static! {
    static ref RUSTC_RESOURCE_SUFFIX: String = load_rustc_resource_suffix()
        .unwrap_or_else(|_| "???".into());
}

fn load_rustc_resource_suffix() -> Result<String, failure::Error> {
    let conn = crate::db::connect_db()?;

    let res = conn.query(
        "SELECT value FROM config WHERE name = 'rustc_version';",
        &[],
    )?;
    if res.is_empty() {
        failure::bail!("missing rustc version");
    }

    if let Some(Ok(vers)) = res.get(0).get_opt::<_, Json>("value") {
        if let Some(vers_str) = vers.as_string() {
            return Ok(crate::utils::parse_rustc_version(vers_str)?);
        }
    }

    failure::bail!("failed to parse the rustc version");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobalAlert {
    pub(crate) url: &'static str,
    pub(crate) text: &'static str,
    pub(crate) css_class: &'static str,
    pub(crate) fa_icon: &'static str,
}

impl ToJson for GlobalAlert {
    fn to_json(&self) -> Json {
        let mut map = BTreeMap::new();
        map.insert("url".to_string(), self.url.to_json());
        map.insert("text".to_string(), self.text.to_json());
        map.insert("css_class".to_string(), self.css_class.to_json());
        map.insert("fa_icon".to_string(), self.fa_icon.to_json());
        Json::Object(map)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T: ToJson> {
    title: Option<String>,
    content: T,
    status: status::Status,
    varss: BTreeMap<String, String>,
    varsb: BTreeMap<String, bool>,
    varsi: BTreeMap<String, i64>,
    rustc_resource_suffix: &'static str,
}

impl<T: ToJson> Page<T> {
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

    /// Sets an integer variable
    pub fn set_int(mut self, var: &str, val: i64) -> Page<T> {
        self.varsi.insert(var.to_owned(), val);
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

impl<T: ToJson> ToJson for Page<T> {
    fn to_json(&self) -> Json {
        let mut tree = BTreeMap::new();

        if let Some(ref title) = self.title {
            tree.insert("title".to_owned(), title.to_json());
        }

        tree.insert(
            "has_global_alert".to_owned(),
            crate::GLOBAL_ALERT.is_some().to_json(),
        );
        if let Some(ref global_alert) = crate::GLOBAL_ALERT {
            tree.insert("global_alert".to_owned(), global_alert.to_json());
        }

        tree.insert("content".to_owned(), self.content.to_json());
        tree.insert(
            "rustc_resource_suffix".to_owned(),
            self.rustc_resource_suffix.to_json(),
        );
        tree.insert(
            "cratesfyi_version".to_owned(),
            crate::BUILD_VERSION.to_json(),
        );
        tree.insert(
            "cratesfyi_version_safe".to_owned(),
            crate::BUILD_VERSION
                .replace(" ", "-")
                .replace("(", "")
                .replace(")", "")
                .to_json(),
        );
        tree.insert("varss".to_owned(), self.varss.to_json());
        tree.insert("varsb".to_owned(), self.varsb.to_json());
        tree.insert("varsi".to_owned(), self.varsi.to_json());
        Json::Object(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::super::releases::{self, Release};
    use super::*;
    use rustc_serialize::json::Json;

    #[test]
    fn serialize_page() {
        let time = time::get_time();

        let mut release = Release::default();
        release.name = "lasso".into();
        release.version = "0.1.0".into();
        release.release_time = time.clone();

        let mut varss = BTreeMap::new();
        varss.insert("test".into(), "works".into());
        let mut varsb = BTreeMap::new();
        varsb.insert("test2".into(), true);
        let mut varsi = BTreeMap::new();
        varsi.insert("test3".into(), 1337);

        let page = Page {
            title: None,
            content: vec![release.clone()],
            status: status::Status::Ok,
            varss,
            varsb,
            varsi,
            rustc_resource_suffix: &*RUSTC_RESOURCE_SUFFIX,
        };

        let correct_json = format!(
            r#"{{
                "content": [{{
                    "name": "lasso",
                    "version": "0.1.0",
                    "description": null,
                    "target_name": null,
                    "rustdoc_status": false,
                    "release_time": "{}",
                    "release_time_rfc3339": "{}",
                    "stars": 0
                }}],
                "varss": {{ "test": "works" }},
                "varsb": {{ "test2": true }},
                "varsi": {{ "test3": 1337 }},
                "rustc_resource_suffix": "{}",
                "cratesfyi_version": "{}",
                "cratesfyi_version_safe": "{}",
                "has_global_alert": {}
            }}"#,
            super::super::duration_to_str(time.clone()),
            time::at(time).rfc3339().to_string(),
            &*RUSTC_RESOURCE_SUFFIX,
            crate::BUILD_VERSION,
            crate::BUILD_VERSION
                .replace(" ", "-")
                .replace("(", "")
                .replace(")", ""),
            crate::GLOBAL_ALERT.is_some(),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes everything in the respective `varsi` unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            page.to_json().to_string()
        );
    }

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
    fn serialize_global_alert() {
        let alert = GlobalAlert {
            url: "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            text: "THE WORLD IS ENDING",
            css_class: "THE END IS NEAR",
            fa_icon: "https://gph.is/1uOvmqR",
        };

        let correct_json = Json::from_str(
            r#"{
            "url": "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            "text": "THE WORLD IS ENDING",
            "css_class": "THE END IS NEAR",
            "fa_icon": "https://gph.is/1uOvmqR"
        }"#,
        )
        .unwrap();

        assert_eq!(correct_json, alert.to_json());
    }
}
