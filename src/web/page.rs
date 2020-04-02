//! Generic page struct

use std::collections::BTreeMap;
use rustc_serialize::json::{Json, ToJson};
use iron::{IronResult, Set, status};
use iron::response::Response;
use handlebars_iron::Template;

lazy_static::lazy_static! {
    static ref RUSTC_RESOURCE_SUFFIX: String = load_rustc_resource_suffix()
        .unwrap_or_else(|_| "???".into());
}

fn load_rustc_resource_suffix() -> Result<String, failure::Error> {
    let conn = crate::db::connect_db()?;

    let res = conn.query("SELECT value FROM config WHERE name = 'rustc_version';", &[])?;
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

        tree.insert("has_global_alert".to_owned(), crate::GLOBAL_ALERT.is_some().to_json());
        if let Some(ref global_alert) = crate::GLOBAL_ALERT {
            tree.insert("global_alert".to_owned(), global_alert.to_json());
        }

        tree.insert("content".to_owned(), self.content.to_json());
        tree.insert("rustc_resource_suffix".to_owned(), self.rustc_resource_suffix.to_json());
        tree.insert("cratesfyi_version".to_owned(), crate::BUILD_VERSION.to_json());
        tree.insert(
            "cratesfyi_version_safe".to_owned(),
            crate::BUILD_VERSION
                .replace(" ", "-")
                .replace("(", "")
                .replace(")", "")
                .to_json()
        );
        tree.insert("varss".to_owned(), self.varss.to_json());
        tree.insert("varsb".to_owned(), self.varsb.to_json());
        tree.insert("varsi".to_owned(), self.varsi.to_json());
        Json::Object(tree)
    }
}
