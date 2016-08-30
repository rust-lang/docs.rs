//! rustdoc handler


use super::pool::Pool;
use super::file::File;
use super::MetaData;
use iron::prelude::*;
use iron::{status, Url};
use iron::modifiers::Redirect;
use router::Router;
use super::match_version;
use super::error::Nope;
use super::page::Page;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;



#[derive(Debug)]
struct RustdocPage {
    pub head: String,
    pub body: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub metadata: Option<MetaData>,
    pub platforms: Option<Json>
}


impl Default for RustdocPage {
    fn default() -> RustdocPage {
        RustdocPage {
            head: String::new(),
            body: String::new(),
            name: String::new(),
            version: String::new(),
            description: None,
            metadata: None,
            platforms: None,
        }
    }
}


impl ToJson for RustdocPage {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("rustdoc_head".to_string(), self.head.to_json());
        m.insert("rustdoc_body".to_string(), self.body.to_json());
        m.insert("rustdoc_status".to_string(), true.to_json());
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("metadata".to_string(), self.metadata.to_json());
        m.insert("platforms".to_string(), self.platforms.to_json());
        m.to_json()
    }
}



pub fn rustdoc_redirector_handler(req: &mut Request) -> IronResult<Response> {

    fn redirect_to_doc(req: &Request,
                       name: &str,
                       vers: &str,
                       target_name: &str)
                       -> IronResult<Response> {
        let url = Url::parse(&format!("{}://{}:{}/{}/{}/{}/",
                                      req.url.scheme,
                                      req.url.host,
                                      req.url.port,
                                      name,
                                      vers,
                                      target_name)[..])
            .unwrap();
        let mut resp = Response::with((status::Found, Redirect(url)));

        use iron::headers::{Expires, HttpDate};
        use time;
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    // this handler should never called without crate pattern
    let crate_name = req.extensions.get::<Router>().unwrap().find("crate").unwrap();
    let req_version = req.extensions.get::<Router>().unwrap().find("version");

    let conn = req.extensions.get::<Pool>().unwrap();

    let version = match match_version(&conn, &crate_name, req_version) {
        Some(v) => v,
        None => return Err(IronError::new(Nope::CrateNotFound, status::NotFound)),
    };

    // get target name
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    let target_name: String = conn.query("SELECT target_name FROM releases INNER JOIN crates ON \
                                          crates.id = releases.crate_id WHERE crates.name = $1 \
                                          AND releases.version = $2",
               &[&crate_name, &version])
        .unwrap()
        .get(0)
        .get(0);

    redirect_to_doc(req, &crate_name, &version, &target_name)
}


pub fn rustdoc_html_server_handler(req: &mut Request) -> IronResult<Response> {

    let name = req.extensions.get::<Router>().unwrap().find("crate").unwrap_or("").to_string();
    let version = req.extensions
        .get::<Router>()
        .unwrap()
        .find("version");
    let conn = req.extensions.get::<Pool>().unwrap();
    let version = try!(match_version(&conn, &name, version)
        .ok_or(IronError::new(Nope::ResourceNotFound, status::NotFound)));

    // remove name and version from path
    for _ in 0..2 {
        req.url.path.remove(0);
    }

    // docs have "rustdoc" prefix in database
    req.url.path.insert(0, "rustdoc".to_owned());

    // add crate name and version
    req.url.path.insert(1, name.clone());
    req.url.path.insert(2, version.clone());

    let path = {
        let mut path = req.url.path.join("/");
        if path.ends_with("/") {
            path.push_str("index.html");
            req.url.path.push("index.html".to_owned());
        }
        path
    };
    
    // don't touch anything other than html files
    if !path.ends_with(".html") {
        return Err(IronError::new(Nope::ResourceNotFound, status::NotFound));
    }


    let file = match File::from_path(&conn, &path) {
        Some(f) => f,
        None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
    };

    let (mut in_head, mut in_body) = (false, false);

    let mut content = RustdocPage::default();

    let file_content = String::from_utf8(file.content).unwrap();

    for line in file_content.lines() {

        if line.starts_with("<head") {
            in_head = true;
            continue;
        } else if line.starts_with("</head") {
            in_head = false;
        } else if line.starts_with("<body") {
            in_body = true;
            continue;
        } else if line.starts_with("</body") {
            in_body = false;
        }

        if in_head {
            content.head.push_str(&line[..]);
            content.head.push('\n');
        } else if in_body {
            content.body.push_str(&line[..]);
            content.body.push('\n');
        }
    }

    // content.metadata = MetaData::from_crate(&conn, &name, &version);
    let (metadata, platforms) = {
        let rows = conn.query("SELECT crates.name,
                                      releases.version,
                                      releases.description,
                                      releases.target_name,
                                      releases.rustdoc_status,
                                      doc_targets
                               FROM releases
                               INNER JOIN crates ON crates.id = releases.crate_id
                               WHERE crates.name = $1 AND releases.version = $2",
                              &[&name, &version]).unwrap();

        let metadata = MetaData {
            name: rows.get(0).get(0),
            version: rows.get(0).get(1),
            description: rows.get(0).get(2),
            target_name: rows.get(0).get(3),
            rustdoc_status: rows.get(0).get(4),
        };
        let platforms: Json = rows.get(0).get(5);
        (Some(metadata), platforms)
    };

    content.metadata = metadata;
    content.platforms = Some(platforms);

    Page::new(content)
        .set_true("show_package_navigation")
        .set_true("package_navigation_documentation_tab")
        .set_true("package_navigation_show_platforms_tab")
        .to_resp("rustdoc")
}



pub fn badge_handler(req: &mut Request) -> IronResult<Response> {
    use iron::headers::ContentType;
    use params::{Params, Value};
    use badge::{Badge, BadgeOptions};

    let version = {
        let params = req.get_ref::<Params>().unwrap();
        match params.find(&["version"]) {
            Some(&Value::String(ref version)) => version.clone(),
            _ => "*".to_owned(),
        }
    };

    let name = req.extensions.get::<Router>().unwrap().find("crate").unwrap();
    let conn = req.extensions.get::<Pool>().unwrap();

    let options = match match_version(&conn, &name, Some(&version)) {
        Some(version) => {
            let rows = conn.query("SELECT rustdoc_status
                                   FROM releases
                                   INNER JOIN crates ON crates.id = releases.crate_id
                                   WHERE crates.name = $1 AND releases.version = $2",
                                  &[&name, &version]).unwrap();
            if rows.len() > 0 && rows.get(0).get(0) {
                BadgeOptions {
                    subject: "docs".to_owned(),
                    status: version,
                    color: "#4d76ae".to_owned(),
                }
            } else {
                BadgeOptions {
                    subject: "docs".to_owned(),
                    status: version,
                    color: "#e05d44".to_owned(),
                }
            }
        },
        None => {
            BadgeOptions {
                subject: "docs".to_owned(),
                status: "no builds".to_owned(),
                color: "#e05d44".to_owned(),
            }
        },
    };

    let mut resp = Response::with((status::Ok, Badge::new(options).unwrap().to_svg()));
    resp.headers.set(ContentType("image/svg+xml".parse().unwrap()));
    Ok(resp)
}
