//! rustdoc handler


use super::pool::Pool;
use super::file::File;
use super::latest_version;
use super::crate_details::CrateDetails;
use iron::prelude::*;
use iron::{status, Url};
use iron::modifiers::Redirect;
use router::Router;
use super::match_version;
use super::error::Nope;
use super::page::Page;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;
use iron::headers::{Expires, HttpDate, CacheControl, CacheDirective};
use time;



#[derive(Debug)]
struct RustdocPage {
    pub head: String,
    pub body: String,
    pub name: String,
    pub full: String,
    pub version: String,
    pub description: Option<String>,
    pub crate_details: Option<CrateDetails>,
}


impl Default for RustdocPage {
    fn default() -> RustdocPage {
        RustdocPage {
            head: String::new(),
            body: String::new(),
            name: String::new(),
            full: String::new(),
            version: String::new(),
            description: None,
            crate_details: None,
        }
    }
}


impl ToJson for RustdocPage {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("rustdoc_head".to_string(), self.head.to_json());
        m.insert("rustdoc_body".to_string(), self.body.to_json());
        m.insert("rustdoc_full".to_string(), self.full.to_json());
        m.insert("rustdoc_status".to_string(), true.to_json());
        m.insert("name".to_string(), self.name.to_json());
        m.insert("version".to_string(), self.version.to_json());
        m.insert("description".to_string(), self.description.to_json());
        m.insert("crate_details".to_string(), self.crate_details.to_json());
        m.to_json()
    }
}



pub fn rustdoc_redirector_handler(req: &mut Request) -> IronResult<Response> {

    fn redirect_to_doc(req: &Request,
                       name: &str,
                       vers: &str,
                       target_name: &str)
                       -> IronResult<Response> {
        let url = ctry!(Url::parse(&format!("{}://{}:{}/{}/{}/{}/",
                                            req.url.scheme(),
                                            req.url.host(),
                                            req.url.port(),
                                            name,
                                            vers,
                                            target_name)[..]));
        let mut resp = Response::with((status::Found, Redirect(url)));
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }

    let router = extension!(req, Router);
    // this handler should never called without crate pattern
    let crate_name = cexpect!(router.find("crate"));
    let req_version = router.find("version");

    let conn = extension!(req, Pool);

    let version = match match_version(&conn, &crate_name, req_version) {
        Some(v) => v,
        None => return Err(IronError::new(Nope::CrateNotFound, status::NotFound)),
    };

    // get target name
    // FIXME: This is a bit inefficient but allowing us to use less code in general
    let target_name: String =
        ctry!(conn.query("SELECT target_name
                          FROM releases
                          INNER JOIN crates ON crates.id = releases.crate_id
                          WHERE crates.name = $1 AND releases.version = $2",
                         &[&crate_name, &version]))
            .get(0)
            .get(0);

    redirect_to_doc(req, &crate_name, &version, &target_name)
}


pub fn rustdoc_html_server_handler(req: &mut Request) -> IronResult<Response> {

    let router = extension!(req, Router);
    let name = router.find("crate").unwrap_or("").to_string();
    let version = router.find("version");
    let conn = extension!(req, Pool);
    let version = try!(match_version(&conn, &name, version)
        .ok_or(IronError::new(Nope::ResourceNotFound, status::NotFound)));
    let mut req_path = req.url.path();

    // remove name and version from path
    for _ in 0..2 {
        req_path.remove(0);
    }

    // docs have "rustdoc" prefix in database
    req_path.insert(0, "rustdoc");

    // add crate name and version
    req_path.insert(1, &name);
    req_path.insert(2, &version);

    let path = {
        let mut path = req_path.join("/");
        if path.ends_with("/") {
            path.push_str("index.html");
            req_path.push("index.html");
        }
        path
    };

    let file = match File::from_path(&conn, &path) {
        Some(f) => f,
        None => return Err(IronError::new(Nope::ResourceNotFound, status::NotFound)),
    };

    // serve file directly if it's not html
    if !path.ends_with(".html") {
        return Ok(file.serve());
    }

    let (mut in_head, mut in_body) = (false, false);

    let mut content = RustdocPage::default();

    let file_content = ctry!(String::from_utf8(file.content));

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

    content.full = file_content;
    let crate_details = cexpect!(CrateDetails::new(&conn, &name, &version));
    let latest_version = latest_version(&crate_details.versions, &version);

    content.crate_details = Some(crate_details);

    Page::new(content)
        .set_true("show_package_navigation")
        .set_true("package_navigation_documentation_tab")
        .set_true("package_navigation_show_platforms_tab")
        .set_bool("is_latest_version", latest_version.is_none())
        .set("latest_version", &latest_version.unwrap_or(String::new()))
        .to_resp("rustdoc")
}



pub fn badge_handler(req: &mut Request) -> IronResult<Response> {
    use iron::headers::ContentType;
    use params::{Params, Value};
    use badge::{Badge, BadgeOptions};

    let version = {
        let params = ctry!(req.get_ref::<Params>());
        match params.find(&["version"]) {
            Some(&Value::String(ref version)) => version.clone(),
            _ => "*".to_owned(),
        }
    };

    let name = cexpect!(extension!(req, Router).find("crate"));
    let conn = extension!(req, Pool);

    let options = match match_version(&conn, &name, Some(&version)) {
        Some(version) => {
            let rows = ctry!(conn.query("SELECT rustdoc_status
                                         FROM releases
                                         INNER JOIN crates ON crates.id = releases.crate_id
                                         WHERE crates.name = $1 AND releases.version = $2",
                                        &[&name, &version]));
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
        }
        None => {
            BadgeOptions {
                subject: "docs".to_owned(),
                status: "no builds".to_owned(),
                color: "#e05d44".to_owned(),
            }
        }
    };

    let mut resp = Response::with((status::Ok, ctry!(Badge::new(options)).to_svg()));
    resp.headers.set(ContentType("image/svg+xml".parse().unwrap()));
    resp.headers.set(Expires(HttpDate(time::now())));
    resp.headers.set(CacheControl(vec![CacheDirective::NoCache,
                                       CacheDirective::NoStore,
                                       CacheDirective::MustRevalidate]));
    Ok(resp)
}
