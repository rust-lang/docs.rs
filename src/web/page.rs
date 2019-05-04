//! Generic page struct

use handlebars_iron::Template;
use iron::response::Response;
use iron::{status, IronResult, Set};
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

pub struct Page<T: ToJson> {
    title: Option<String>,
    content: T,
    status: status::Status,
    varss: BTreeMap<String, String>,
    varsb: BTreeMap<String, bool>,
    varsi: BTreeMap<String, i64>,
}

impl<T: ToJson> Page<T> {
    pub fn new(content: T) -> Page<T> {
        Page {
            title: None,
            content: content,
            status: status::Ok,
            varss: BTreeMap::new(),
            varsb: BTreeMap::new(),
            varsi: BTreeMap::new(),
        }
    }

    /// Sets a string variable
    pub fn set(mut self, var: &str, val: &str) -> Page<T> {
        &self.varss.insert(var.to_owned(), val.to_owned());
        self
    }

    /// Sets a boolean variable
    pub fn set_bool(mut self, var: &str, val: bool) -> Page<T> {
        &self.varsb.insert(var.to_owned(), val);
        self
    }

    /// Sets a boolean variable to true
    pub fn set_true(mut self, var: &str) -> Page<T> {
        &self.varsb.insert(var.to_owned(), true);
        self
    }

    /// Sets an integer variable
    pub fn set_int(mut self, var: &str, val: i64) -> Page<T> {
        &self.varsi.insert(var.to_owned(), val);
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

        tree.insert("content".to_owned(), self.content.to_json());
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
