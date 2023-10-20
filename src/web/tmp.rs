use crate::db::types::BuildStatus;
use crate::docbuilder::Limits;
use crate::error::Result;
use crate::web::crate_details::CrateDetails;
use crate::web::headers::CanonicalUrl;
use crate::web::page::templates::filters;
use crate::web::rustdoc::RustdocPage;
use crate::web::MetaData;
use anyhow::Context;
use chrono::{DateTime, Utc};
use rinja::Template;
use serde::{Deserialize, Serialize};
use std::{fmt, ops::Deref, sync::Arc};
use tracing::trace;

/*pub struct Topbar<'a> {
    inner: &'a RustdocPage,
}

impl<'a> Deref for Topbar<'a> {
    type Target = RustdocPage;

    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BuildDetails {
    id: i32,
    rustc_version: Option<String>,
    docsrs_version: Option<String>,
    build_status: BuildStatus,
    build_time: Option<DateTime<Utc>>,
    output: String,
    errors: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BuildDetailsPage {
    metadata: MetaData,
    build_details: BuildDetails,
    use_direct_platform_links: bool,
    all_log_filenames: Vec<String>,
    current_filename: Option<String>,
    csp_nonce: String,
}

impl BuildDetailsPage {
    pub(crate) fn krate(&self) -> Option<&CrateDetails> {
        None
    }

    pub(crate) fn permalink_path(&self) -> &str {
        ""
    }
}*/

#[derive(Debug, Clone, PartialEq, Serialize)]
struct CrateDetailsPage {
    details: CrateDetails,
    csp_nonce: String,
}

impl CrateDetailsPage {
    pub(crate) fn krate(&self) -> Option<&CrateDetails> {
        Some(&self.details)
    }

    pub(crate) fn permalink_path(&self) -> &str {
        ""
    }
}

impl ::rinja::Template for CrateDetailsPage {
    fn render_into(&self, writer: &mut (impl ::std::fmt::Write + ?Sized)) -> ::rinja::Result<()> {
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/clipboard.svg");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/macros.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/rustdoc/platforms.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/global_alert.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/theme.js");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/topbar_end.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/topbar_begin.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/base.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/rustdoc/topbar.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/topbar.html");
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/crate/details.html");
        const _: &[::core::primitive::u8] = ::core::include_bytes!(
            "/home/imperio/rust/docs.rs/templates/header/package_navigation.html"
        );
        ::std::write!(
writer,
"<!DOCTYPE html>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"UTF-8\">\n        <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n        <meta name=\"generator\" content=\"docs.rs {expr0}\">",
expr0 = &::rinja::MarkupDisplay::new_unsafe(&(crate::BUILD_VERSION), ::rinja::Html),
)?;
        ::std::write!(
writer,
"<link rel=\"canonical\" href=\"https://docs.rs/crate/{expr1}/latest\" />\n        <link rel=\"stylesheet\" href=\"/-/static/vendored.css?{expr2}\" media=\"all\" />\n        <link rel=\"stylesheet\" href=\"/-/static/style.css?{expr2}\" media=\"all\" />\n\n        <link rel=\"search\" href=\"/-/static/opensearch.xml\" type=\"application/opensearchdescription+xml\" title=\"Docs.rs\" />\n\n        <title>",
expr1 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr2 = &::rinja::MarkupDisplay::new_unsafe(&(filters::slugify(crate::BUILD_VERSION)?), ::rinja::Html),
)?;
        {
            let (name, version) = ((&self.details.name), (&self.details.version));
            if *(&(!name.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "{expr0} {expr1} - Docs.rs",
                    expr0 = &::rinja::MarkupDisplay::new_unsafe(&(name), ::rinja::Html),
                    expr1 = &::rinja::MarkupDisplay::new_unsafe(&(version), ::rinja::Html),
                )?;
            } else {
                writer.write_str("Docs.rs")?;
            }
        }
        ::std::write!(
            writer,
            "</title>\n\n        <script nonce=\"{expr3}\">",
            expr3 = &::rinja::MarkupDisplay::new_unsafe(&(self.csp_nonce), ::rinja::Html),
        )?;
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/theme.js");
        writer.write_str("(function() {\n    function applyTheme(theme) {\n        if (theme) {\n            document.documentElement.dataset.docsRsTheme = theme;\n        }\n    }\n\n    window.addEventListener(\"storage\", ev => {\n        if (ev.key === \"rustdoc-theme\") {\n            applyTheme(ev.newValue);\n        }\n    });\n\n    // see ./storage-change-detection.html for details\n    window.addEventListener(\"message\", ev => {\n        if (ev.data && ev.data.storage && ev.data.storage.key === \"rustdoc-theme\") {\n            applyTheme(ev.data.storage.value);\n        }\n    });\n\n    applyTheme(window.localStorage.getItem(\"rustdoc-theme\"));\n})();")?;
        writer.write_str("</script>")?;
        ::std::write!(
writer,
"<script defer type=\"text/javascript\" nonce=\"{expr4}\" src=\"/-/static/menu.js?{expr5}\"></script>\n        <script defer type=\"text/javascript\" nonce=\"{expr4}\" src=\"/-/static/index.js?{expr5}\"></script>\n    </head>\n\n    <body class=\"",
expr4 = &::rinja::MarkupDisplay::new_unsafe(&(self.csp_nonce), ::rinja::Html),expr5 = &::rinja::MarkupDisplay::new_unsafe(&(filters::slugify(crate::BUILD_VERSION)?), ::rinja::Html),
)?;
        writer.write_str("\">")?;
        let current_target = String::new();
        let metadata = self.details.metadata;
        let latest_version = "";
        let latest_path = "";
        let target = "";
        let inner_path = self.details.metadata.target_name_url();
        let is_latest_version = true;
        let is_prerelease = false;
        let use_direct_platform_links = true;
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/rustdoc/topbar.html");
        let search_query = Some(String::new());
        writer.write_str("\n\n")?;
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/topbar_begin.html");
        writer.write_str("\n<div class=\"nav-container\">\n    <div class=\"container\">\n        <div class=\"pure-menu pure-menu-horizontal\" role=\"navigation\" aria-label=\"Main navigation\">\n            <form action=\"/releases/search\"\n                  method=\"GET\"\n                  id=\"nav-search-form\"\n                  class=\"landing-search-form-nav ")?;
        if *(&(!is_latest_version) as &bool) {
            writer.write_str("not-latest")?;
        }
        writer.write_str(" ")?;
        if *(&(metadata.yanked.unwrap_or_default()) as &bool) {
            writer.write_str("yanked")?;
        }
        ::std::write!(
writer,
"\">\n\n                \n                <a href=\"/\" class=\"pure-menu-heading pure-menu-link docsrs-logo\" aria-label=\"Docs.rs\">\n                    <span title=\"Docs.rs\">{expr0}</span>\n                    <span class=\"title\">Docs.rs</span>\n                </a>",
expr0 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cubes", false, false, "")?), ::rinja::Html),
)?;
        let crate_url = ::std::format!("/crate/{}/{}", &(metadata.name), &(metadata.req_version));
        let rest_menu_url = filters::rest_menu_url(&(current_target), &(inner_path))?;
        let platform_menu_url =
            ::std::format!("{}/menus/platforms{}", &(crate_url), &(rest_menu_url));
        let releases_menu_url =
            ::std::format!("{}/menus/releases{}", &(crate_url), &(rest_menu_url));
        ::std::write!(
writer,
"<ul class=\"pure-menu-list\">\n    <script id=\"crate-metadata\" type=\"application/json\">\n        \n        {{\n            \"name\": {expr0},\n            \"version\": {expr1}\n        }}\n    </script>",
expr0 = &::rinja::filters::safe(::rinja::Html, &(filters::json_encode(&(metadata.name))?))?,expr1 = &::rinja::filters::safe(::rinja::Html, &(filters::json_encode(&(metadata.version))?))?,
)?;
        if let Some(krate) = &self.krate() {
            ::std::write!(
writer,
"<li class=\"pure-menu-item pure-menu-has-children\">\n            <a href=\"#\" class=\"pure-menu-link crate-name\" title=\"{expr2}\">\n                {expr3}\n                <span class=\"title\">{expr4}-{expr5}</span>\n            </a>\n\n            \n            <div class=\"pure-menu-children package-details-menu\">\n                \n                <ul class=\"pure-menu-list menu-item-divided\">\n                    <li class=\"pure-menu-heading\" id=\"crate-title\">\n                        {expr4} {expr5}\n                        <span id=\"clipboard\" class=\"fa-svg fa-svg-fw\" title=\"Copy crate name and version information\">",
expr2 = &::rinja::MarkupDisplay::new_unsafe(&(krate.description.as_deref().unwrap_or_default()), ::rinja::Html),expr3 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cube", false, false, "")?), ::rinja::Html),expr4 = &::rinja::MarkupDisplay::new_unsafe(&(krate.name), ::rinja::Html),expr5 = &::rinja::MarkupDisplay::new_unsafe(&(krate.version), ::rinja::Html),
)?;
            const _: &[::core::primitive::u8] =
                ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/clipboard.svg");
            writer.write_str("<svg width=\"24\" height=\"25\" viewBox=\"0 0 24 25\" fill=\"currentColor\" xmlns=\"http://www.w3.org/2000/svg\" aria-label=\"Copy to clipboard\"><path d=\"M18 20h2v3c0 1-1 2-2 2H2c-.998 0-2-1-2-2V5c0-.911.755-1.667 1.667-1.667h5A3.323 3.323 0 0110 0a3.323 3.323 0 013.333 3.333h5C19.245 3.333 20 4.09 20 5v8.333h-2V9H2v14h16v-3zM3 7h14c0-.911-.793-1.667-1.75-1.667H13.5c-.957 0-1.75-.755-1.75-1.666C11.75 2.755 10.957 2 10 2s-1.75.755-1.75 1.667c0 .911-.793 1.666-1.75 1.666H4.75C3.793 5.333 3 6.09 3 7z\"/><path d=\"M4 19h6v2H4zM12 11H4v2h8zM4 17h4v-2H4zM15 15v-3l-4.5 4.5L15 21v-3l8.027-.032L23 15z\"/></svg>")?;
            writer.write_str("</span>\n                    </li>")?;
            if *(&(metadata.req_version.to_string() == "latest") as &bool) {
                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                        <a href=\"{expr6}\" class=\"pure-menu-link description\" id=\"permalink\" title=\"Get a link to this specific version\">\n                            {expr7} Permalink\n                        </a>\n                    </li>",
expr6 = &::rinja::filters::safe(::rinja::Html, {
self.permalink_path()}
)?,expr7 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("link", false, false, "")?), ::rinja::Html),
)?;
            }
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                        <a href=\"{expr8}\" class=\"pure-menu-link description\" title=\"See {expr9} in docs.rs\">\n                            {expr10} Docs.rs crate page\n                        </a>\n                    </li>\n\n                    <li class=\"pure-menu-item\">\n                        <a href=\"{expr8}\" class=\"pure-menu-link\">\n                            {expr11} {expr12}\n                        </a>\n                    </li>\n                </ul>\n\n                <div class=\"pure-g menu-item-divided\">\n                    <div class=\"pure-u-1-2 right-border\">\n                        <ul class=\"pure-menu-list\">\n                            <li class=\"pure-menu-heading\">Links</li>\n\n                            ",
expr8 = &::rinja::filters::safe(::rinja::Html, &(crate_url))?,expr9 = &::rinja::MarkupDisplay::new_unsafe(&(krate.name), ::rinja::Html),expr10 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cube", false, false, "")?), ::rinja::Html),expr11 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("scale-unbalanced-flip", false, false, "")?), ::rinja::Html),expr12 = &::rinja::MarkupDisplay::new_unsafe(&(krate.license.as_deref().unwrap_or_default()), ::rinja::Html),
)?;
            if let Some(homepage_url) = &krate.homepage_url {
                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                    <a href=\"{expr13}\" class=\"pure-menu-link\">\n                                        {expr14} Homepage\n                                    </a>\n                                </li>",
expr13 = &::rinja::MarkupDisplay::new_unsafe(&(homepage_url), ::rinja::Html),expr14 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("house", false, false, "")?), ::rinja::Html),
)?;
            }
            if let Some(documentation_url) = &krate.documentation_url {
                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                    <a href=\"{expr15}\" title=\"Canonical documentation\" class=\"pure-menu-link\">\n                                        {expr16} Documentation\n                                    </a>\n                                </li>",
expr15 = &::rinja::MarkupDisplay::new_unsafe(&(documentation_url), ::rinja::Html),expr16 = &::rinja::MarkupDisplay::new_unsafe(&(filters::far("file-lines", false, false, "")?), ::rinja::Html),
)?;
            }
            if let Some(repository_url) = &krate.repository_url {
                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                    <a href=\"{expr17}\" class=\"pure-menu-link\">\n                                        {expr18} Repository\n                                    </a>\n                                </li>",
expr17 = &::rinja::MarkupDisplay::new_unsafe(&(repository_url), ::rinja::Html),expr18 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("code-branch", false, false, "")?), ::rinja::Html),
)?;
            }
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                <a href=\"https://crates.io/crates/{expr19}\" class=\"pure-menu-link\" title=\"See {expr19} in crates.io\">\n                                    {expr20} Crates.io\n                                </a>\n                            </li>\n\n                            \n                            <li class=\"pure-menu-item\">\n                                <a href=\"{expr21}/source/\" title=\"Browse source of {expr22}-{expr23}\" class=\"pure-menu-link\">\n                                    {expr24} Source\n                                </a>\n                            </li>\n                        </ul>\n                    </div>\n\n                    \n                    <div class=\"pure-u-1-2\">\n                        <ul class=\"pure-menu-list\" id=\"topbar-owners\">\n                            <li class=\"pure-menu-heading\">Owners</li>",
expr19 = &::rinja::MarkupDisplay::new_unsafe(&(krate.name), ::rinja::Html),expr20 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cube", false, false, "")?), ::rinja::Html),expr21 = &::rinja::filters::safe(::rinja::Html, &(crate_url))?,expr22 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.name), ::rinja::Html),expr23 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.version), ::rinja::Html),expr24 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("folder-open", false, false, "")?), ::rinja::Html),
)?;
            {
                let _iter = (&krate.owners).into_iter();
                for (owner, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                    ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                    <a href=\"https://crates.io/{expr25}s/{expr26}\" class=\"pure-menu-link\">\n                                        {expr27} {expr26}\n                                    </a>\n                                </li>",
expr25 = &::rinja::MarkupDisplay::new_unsafe(&(owner.2), ::rinja::Html),expr26 = &::rinja::MarkupDisplay::new_unsafe(&(owner.0), ::rinja::Html),expr27 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("user", false, false, "")?), ::rinja::Html),
)?;
                }
            }
            writer.write_str("</ul>\n                    </div>\n                </div>\n\n                <div class=\"pure-g menu-item-divided\">\n                    <div class=\"pure-u-1-2 right-border\">\n                        <ul class=\"pure-menu-list\">\n                            <li class=\"pure-menu-heading\">Dependencies</li>\n\n                            \n                            <li class=\"pure-menu-item\">\n                                <div class=\"pure-menu pure-menu-scrollable sub-menu\" tabindex=\"-1\">\n                                    <ul class=\"pure-menu-list\">")?;
            {
                let _iter = (&krate.dependencies).into_iter();
                for (dep, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                    if let serde_json::Value::Array(dep) = &dep {
                        ::std::write!(
writer,
"\n                                                <li class=\"pure-menu-item\">\n                                                    <a href=\"/{expr28}/{expr29}\" class=\"pure-menu-link\">\n                                                        {expr28} {expr29}\n                                                        ",
expr28 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[0]), ::rinja::Html),expr29 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[1]), ::rinja::Html),
)?;
                        if *(&(dep.len() > 2) as &bool) {
                            ::std::write!(
writer,
"\n                                                            <i class=\"dependencies {expr30}\">{expr30}</i>\n                                                            ",
expr30 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[2]), ::rinja::Html),
)?;
                            if *(&(dep.len() > 3) as &bool) {
                                writer.write_str("\n                                                                <i>optional</i>\n                                                            ")?;
                            }
                            writer.write_str(
                                "\n                                                        ",
                            )?;
                        } else {
                            writer.write_str("\n                                                            <i class=\"dependencies\"></i>\n                                                        ")?;
                        }
                        writer.write_str("\n                                                    </a>\n                                                </li>\n                                            ")?;
                    }
                }
            }
            ::std::write!(
writer,
"</ul>\n                                </div>\n                            </li>\n                        </ul>\n                    </div>\n\n                    <div class=\"pure-u-1-2\">\n                        <ul class=\"pure-menu-list\">\n                            <li class=\"pure-menu-heading\">Versions</li>\n\n                            <li class=\"pure-menu-item\">\n                                <div class=\"pure-menu pure-menu-scrollable sub-menu\" id=\"releases-list\" tabindex=\"-1\" data-url=\"{expr31}\">\n                                    <span class=\"rotate\">{expr32}</span>\n                                </div>\n                            </li>\n                        </ul>\n                    </div>\n                </div>",
expr31 = &::rinja::MarkupDisplay::new_unsafe(&(releases_menu_url), ::rinja::Html),expr32 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("spinner", false, false, "")?), ::rinja::Html),
)?;
            if let (Some(documented), Some(total)) = &(krate.documented_items, krate.total_items) {
                let documented = filters::as_f32(&(documented))?;
                writer.write_str("\n                    ")?;
                let total = filters::as_f32(&(total))?;
                let percent = documented * 100f32 / total;
                ::std::write!(
writer,
"\n                    \n                    <div class=\"pure-g\">\n                        <div class=\"pure-u-1\">\n                            <ul class=\"pure-menu-list\">\n                                <li>\n                                    <a href=\"{expr33}\" class=\"pure-menu-link\">\n                                        <b>{expr34}%</b>\n                                        of the crate is documented\n                                    </a>\n                                </li>\n                            </ul>\n                        </div>\n                    </div>",
expr33 = &::rinja::filters::safe(::rinja::Html, &(crate_url))?,expr34 = &::rinja::MarkupDisplay::new_unsafe(&(filters::round(&(percent), 2)?), ::rinja::Html),
)?;
            }
            writer.write_str("</div>\n        </li>")?;
        } else {
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n            <a href=\"{expr35}\" class=\"pure-menu-link crate-name\" ",
expr35 = &::rinja::filters::safe(::rinja::Html, &(crate_url))?,
)?;
            if let Some(description) = &metadata.description {
                ::std::write!(
                    writer,
                    "title=\"{expr36}\"",
                    expr36 = &::rinja::MarkupDisplay::new_unsafe(&(description), ::rinja::Html),
                )?;
            }
            ::std::write!(
writer,
">\n                {expr37}\n                <span class=\"title\">{expr38}-{expr39}</span>\n            </a>\n        </li>",
expr37 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cube", false, false, "")?), ::rinja::Html),expr38 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.name), ::rinja::Html),expr39 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.version), ::rinja::Html),
)?;
        }
        let yanked = metadata.yanked.unwrap_or_default();
        writer.write_str("\n    ")?;
        if *(&(is_latest_version && yanked) as &bool) {
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n            <span class=\"pure-menu-link warn\">\n                {expr40}\n                <span class=\"title\">This release has been yanked</span>\n            </span>\n        </li>",
expr40 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("triangle-exclamation", false, false, "")?), ::rinja::Html),
)?;
        } else if *(&(!is_latest_version) as &bool) {
            let tooltip = "";
            let title = "";
            writer.write_str("\n        ")?;
            if *(&(yanked) as &bool) {
                let tooltip = ::std::format!("You are seeing a yanked version of the {} crate. Click here to go to the latest version.", &(metadata.name));
                let title = "This release has been yanked, go to latest version";
            } else if *(&(is_prerelease) as &bool) {
                let tooltip = ::std::format!("You are seeing a pre-release version of the {} crate. Click here to go to the latest stable version.", &(metadata.name));
                let title = "Go to latest stable release";
            } else {
                let tooltip = ::std::format!("You are seeing an outdated version of the {} crate. Click here to go to the latest version.", &(metadata.name));
                let title = "Go to latest version";
            }
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n            <a href=\"{expr41}\" class=\"pure-menu-link warn\"\n                data-fragment=\"retain\"\n                title=\"{expr42}\">\n                {expr43}\n                <span class=\"title\">{expr44}</span>\n            </a>\n        </li>",
expr41 = &::rinja::filters::safe(::rinja::Html, &(latest_path))?,expr42 = &::rinja::MarkupDisplay::new_unsafe(&(tooltip), ::rinja::Html),expr43 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("triangle-exclamation", false, false, "")?), ::rinja::Html),expr44 = &::rinja::MarkupDisplay::new_unsafe(&(title), ::rinja::Html),
)?;
        }
        if let Some(doc_targets) = &metadata.doc_targets {
            if *(&(!doc_targets.is_empty()) as &bool) {
                ::std::write!(
writer,
"<li class=\"pure-menu-item pure-menu-has-children\">\n                <a href=\"#\" class=\"pure-menu-link\" aria-label=\"Platform\">\n                    {expr45}\n                    <span class=\"title\">Platform</span>\n                </a>\n\n                \n                <ul class=\"pure-menu-children\" id=\"platforms\" data-url=\"{expr46}\">",
expr45 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("gears", false, false, "")?), ::rinja::Html),expr46 = &::rinja::MarkupDisplay::new_unsafe(&(platform_menu_url), ::rinja::Html),
)?;
                if *(&(doc_targets.len() < crate::DEFAULT_MAX_TARGETS) as &bool) {
                    let use_direct_platform_links = false;
                    const _: &[::core::primitive::u8] = ::core::include_bytes!(
                        "/home/imperio/rust/docs.rs/templates/rustdoc/platforms.html"
                    );
                    if let Some(doc_targets) = &metadata.doc_targets {
                        {
                            let _iter = (doc_targets).into_iter();
                            for (target, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                                let target_no_follow = "";
                                let target_url = String::new();
                                if *(&(use_direct_platform_links) as &bool) {
                                    let target_url = ::std::format!(
                                        "/{}/{}/{}/{}",
                                        &(metadata.name),
                                        &(metadata.req_version),
                                        &(target),
                                        &(inner_path)
                                    );
                                } else {
                                    let target_url = ::std::format!(
                                        "/crate/{}/{}/target-redirect/{}/{}",
                                        &(metadata.name),
                                        &(metadata.req_version),
                                        &(target),
                                        &(inner_path)
                                    );
                                    let target_no_follow = "nofollow";
                                }
                                let current = "";
                                if *(&(current_target == *target) as &bool) {
                                    let current = " current";
                                }
                                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n            <a href=\"{expr0}\" class=\"pure-menu-link{expr1}\" data-fragment=\"retain\" rel=\"{expr2}\">{expr3}</a>\n        </li>",
expr0 = &::rinja::filters::safe(::rinja::Html, &(target_url))?,expr1 = &::rinja::filters::safe(::rinja::Html, &(current))?,expr2 = &::rinja::MarkupDisplay::new_unsafe(&(target_no_follow), ::rinja::Html),expr3 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
)?;
                            }
                        }
                    }
                } else {
                    ::std::write!(
                        writer,
                        "<span class=\"rotate\">{expr47}</span>",
                        expr47 = &::rinja::MarkupDisplay::new_unsafe(
                            &(filters::fas("spinner", false, false, "")?),
                            ::rinja::Html
                        ),
                    )?;
                }
                ::std::write!(
writer,
"</ul>\n            </li><li class=\"pure-menu-item\">\n                <a href=\"{expr48}/features\" title=\"Browse available feature flags of {expr49}-{expr50}\" class=\"pure-menu-link\">\n                    {expr51}\n                    <span class=\"title\">Feature flags</span>\n                </a>\n            </li>",
expr48 = &::rinja::filters::safe(::rinja::Html, &(crate_url))?,expr49 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.name), ::rinja::Html),expr50 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.version), ::rinja::Html),expr51 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("flag", false, false, "")?), ::rinja::Html),
)?;
            }
        }
        writer.write_str("</ul>")?;
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/topbar_end.html");
        writer.write_str("<div class=\"spacer\"></div>\n                \n                ")?;
        const _: &[::core::primitive::u8] =
            ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/header/global_alert.html");
        writer.write_str("\n\n")?;
        if let Some(global_alert) = &crate::GLOBAL_ALERT {
            ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n        <a href=\"{expr0}\" class=\"pure-menu-link {expr1}\">{expr2}\n            {expr3}</a>\n    </li>\n",
expr0 = &::rinja::filters::safe(::rinja::Html, &(global_alert.url))?,expr1 = &::rinja::MarkupDisplay::new_unsafe(&(global_alert.css_class), ::rinja::Html),expr2 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas(&(global_alert.fa_icon), false, false, "")?), ::rinja::Html),expr3 = &::rinja::MarkupDisplay::new_unsafe(&(global_alert.text), ::rinja::Html),
)?;
        }
        writer.write_str("<ul class=\"pure-menu-list\">\n                    <li class=\"pure-menu-item pure-menu-has-children\">\n                        <a href=\"#\" class=\"pure-menu-link\" aria-label=\"Rust\">Rust</a>\n                        <ul class=\"pure-menu-children\">\n                            ")?;
        {
            let (href, text, target) = ((&"/about"), (&"About docs.rs"), (&""));
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr0}\" ",
expr0 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr1}\"",
                    expr1 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr2}\n        </a>\n    </li>\n",
                expr2 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://foundation.rust-lang.org/policies/privacy-policy/#docs.rs"),
                (&"Privacy policy"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr3}\" ",
expr3 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr4}\"",
                    expr4 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr5}\n        </a>\n    </li>\n",
                expr5 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://www.rust-lang.org/"),
                (&"Rust website"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr6}\" ",
expr6 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr7}\"",
                    expr7 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr8}\n        </a>\n    </li>\n",
                expr8 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://doc.rust-lang.org/book/"),
                (&"The Book"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr9}\" ",
expr9 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr10}\"",
                    expr10 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr11}\n        </a>\n    </li>\n",
                expr11 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://doc.rust-lang.org/std/"),
                (&"Standard Library API Reference"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr12}\" ",
expr12 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr13}\"",
                    expr13 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr14}\n        </a>\n    </li>\n",
                expr14 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://doc.rust-lang.org/rust-by-example/"),
                (&"Rust by Example"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr15}\" ",
expr15 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr16}\"",
                    expr16 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr17}\n        </a>\n    </li>\n",
                expr17 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://doc.rust-lang.org/cargo/guide/"),
                (&"The Cargo Guide"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr18}\" ",
expr18 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr19}\"",
                    expr19 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr20}\n        </a>\n    </li>\n",
                expr20 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        writer.write_str("\n\n                            ")?;
        {
            let (href, text, target) = (
                (&"https://doc.rust-lang.org/nightly/clippy"),
                (&"Clippy Documentation"),
                (&"_blank"),
            );
            ::std::write!(
writer,
"\n    <li class=\"pure-menu-item\">\n        <a class=\"pure-menu-link\" href=\"{expr21}\" ",
expr21 = &::rinja::MarkupDisplay::new_unsafe(&(href), ::rinja::Html),
)?;
            if *(&(!target.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    "target=\"{expr22}\"",
                    expr22 = &::rinja::MarkupDisplay::new_unsafe(&(target), ::rinja::Html),
                )?;
            }
            ::std::write!(
                writer,
                ">\n            {expr23}\n        </a>\n    </li>\n",
                expr23 = &::rinja::MarkupDisplay::new_unsafe(&(text), ::rinja::Html),
            )?;
        }
        ::std::write!(
writer,
"\n                        </ul>\n                    </li>\n                </ul>\n                \n                <div id=\"search-input-nav\">\n                    <label for=\"nav-search\">\n                        {expr24}\n                    </label>\n\n                    \n                    \n                    <input id=\"nav-search\" name=\"query\" type=\"text\" aria-label=\"Find crate by search query\" tabindex=\"-1\"\n                        placeholder=\"Find crate\"",
expr24 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("magnifying-glass", false, false, "")?), ::rinja::Html),
)?;
        if let Some(query) = &search_query {
            if *(&(!query.is_empty()) as &bool) {
                ::std::write!(
                    writer,
                    " value=\"{expr25}\"",
                    expr25 = &::rinja::MarkupDisplay::new_unsafe(&(query), ::rinja::Html),
                )?;
            }
        }
        writer.write_str(
            ">\n                </div>\n            </form>\n        </div>\n    </div>\n</div>",
        )?;
        writer.write_str("\n    ")?;
        {
            let (title, metadata, active_tab) = ((&false), (&self.details.metadata), (&"crate"));
            let crate_path = ::std::format!("{}/{}", &(metadata.name), &(metadata.req_version));
            writer.write_str("\n    <div class=\"docsrs-package-container\">\n        <div class=\"container\">\n            <div class=\"description-container\">\n                \n\n                \n                <h1 id=\"crate-title\">")?;
            if *(&(title) as &bool) {
                ::std::write!(
                    writer,
                    "{expr0}",
                    expr0 = &::rinja::MarkupDisplay::new_unsafe(&(title), ::rinja::Html),
                )?;
            } else {
                ::std::write!(
writer,
"{expr1} {expr2}\n                        <span id=\"clipboard\" class=\"fa-svg fa-svg-fw\" title=\"Copy crate name and version information\">",
expr1 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.name), ::rinja::Html),expr2 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.version), ::rinja::Html),
)?;
                const _: &[::core::primitive::u8] =
                    ::core::include_bytes!("/home/imperio/rust/docs.rs/templates/clipboard.svg");
                writer.write_str("<svg width=\"24\" height=\"25\" viewBox=\"0 0 24 25\" fill=\"currentColor\" xmlns=\"http://www.w3.org/2000/svg\" aria-label=\"Copy to clipboard\"><path d=\"M18 20h2v3c0 1-1 2-2 2H2c-.998 0-2-1-2-2V5c0-.911.755-1.667 1.667-1.667h5A3.323 3.323 0 0110 0a3.323 3.323 0 013.333 3.333h5C19.245 3.333 20 4.09 20 5v8.333h-2V9H2v14h16v-3zM3 7h14c0-.911-.793-1.667-1.75-1.667H13.5c-.957 0-1.75-.755-1.75-1.666C11.75 2.755 10.957 2 10 2s-1.75.755-1.75 1.667c0 .911-.793 1.666-1.75 1.666H4.75C3.793 5.333 3 6.09 3 7z\"/><path d=\"M4 19h6v2H4zM12 11H4v2h8zM4 17h4v-2H4zM15 15v-3l-4.5 4.5L15 21v-3l8.027-.032L23 15z\"/></svg>")?;
                writer.write_str("</span>")?;
            }
            writer.write_str(
                "</h1>\n\n                \n                <div class=\"description\">",
            )?;
            if let Some(description) = &metadata.description {
                ::std::write!(
                    writer,
                    "{expr3}",
                    expr3 = &::rinja::MarkupDisplay::new_unsafe(&(description), ::rinja::Html),
                )?;
            }
            ::std::write!(
writer,
"</div>\n\n\n                <div class=\"pure-menu pure-menu-horizontal\">\n                    <ul class=\"pure-menu-list\">\n                        \n                        <li class=\"pure-menu-item\"><a href=\"/crate/{expr4}\"\n                                class=\"pure-menu-link",
expr4 = &::rinja::filters::safe(::rinja::Html, &(crate_path))?,
)?;
            if *(&(active_tab == &"crate") as &bool) {
                writer.write_str(" pure-menu-active")?;
            }
            ::std::write!(
writer,
"\">\n                                {expr5}\n                                <span class=\"title\"> Crate</span>\n                            </a>\n                        </li>\n\n                        \n                        <li class=\"pure-menu-item\">\n                            <a href=\"/crate/{expr6}/source/\"\n                                class=\"pure-menu-link",
expr5 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("cube", false, false, "")?), ::rinja::Html),expr6 = &::rinja::filters::safe(::rinja::Html, &(crate_path))?,
)?;
            if *(&(active_tab == &"source") as &bool) {
                writer.write_str(" pure-menu-active")?;
            }
            ::std::write!(
writer,
"\">\n                                {expr7}\n                                <span class=\"title\"> Source</span>\n                            </a>\n                        </li>\n\n                        \n                        <li class=\"pure-menu-item\">\n                            <a href=\"/crate/{expr8}/builds\"\n                                class=\"pure-menu-link",
expr7 = &::rinja::MarkupDisplay::new_unsafe(&(filters::far("folder-open", false, false, "")?), ::rinja::Html),expr8 = &::rinja::filters::safe(::rinja::Html, &(crate_path))?,
)?;
            if *(&(active_tab == &"builds") as &bool) {
                writer.write_str(" pure-menu-active")?;
            }
            ::std::write!(
writer,
"\">\n                                {expr9}\n                                <span class=\"title\"> Builds</span>\n                            </a>\n                        </li>\n\n                        \n                        <li class=\"pure-menu-item\">\n                            <a href=\"/crate/{expr10}/features\"\n                               class=\"pure-menu-link",
expr9 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("gears", false, false, "")?), ::rinja::Html),expr10 = &::rinja::filters::safe(::rinja::Html, &(crate_path))?,
)?;
            if *(&(active_tab == &"features") as &bool) {
                writer.write_str(" pure-menu-active")?;
            }
            ::std::write!(
writer,
"\">\n                                {expr11}\n                                <span class=\"title\">Feature flags</span>\n                            </a>\n                        </li>\n                    </ul>\n                </div>\n            </div>",
expr11 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("flag", false, false, "")?), ::rinja::Html),
)?;
            if *(&(metadata.rustdoc_status.unwrap_or_default()) as &bool) {
                ::std::write!(
writer,
"<a href=\"/{expr12}/{expr13}/\" class=\"doc-link\">\n                    {expr14} Documentation\n                </a>",
expr12 = &::rinja::filters::safe(::rinja::Html, &(crate_path))?,expr13 = &::rinja::MarkupDisplay::new_unsafe(&(metadata.target_name.as_deref().unwrap_or_default()), ::rinja::Html),expr14 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("book", false, false, "")?), ::rinja::Html),
)?;
            }
            writer.write_str("</div>\n    </div>\n")?;
        }
        writer.write_str("<div class=\"container package-page-container\">\n        <div class=\"pure-g\">\n            <div class=\"pure-u-1 pure-u-sm-7-24 pure-u-md-5-24\">\n                <div class=\"pure-menu package-menu\">\n                    <ul class=\"pure-menu-list\">")?;
        if let (Some(documented), Some(total)) =
            &(self.details.documented_items, self.details.total_items)
        {
            let percent = documented as f32 * 100. / total as f32;
            ::std::write!(
writer,
"\n                            <li class=\"pure-menu-heading\">Coverage</li>\n                            <li class=\"pure-menu-item text-center\"><b>{expr0}%</b><br>\n                                <span class=\"documented-info\"><b>{expr1}</b> out of <b>{expr2}</b> items documented</span>",
expr0 = &::rinja::MarkupDisplay::new_unsafe(&(filters::round(&(percent), 2)?), ::rinja::Html),expr1 = &::rinja::MarkupDisplay::new_unsafe(&(documented), ::rinja::Html),expr2 = &::rinja::MarkupDisplay::new_unsafe(&(total), ::rinja::Html),
)?;
            if let (Some(needing_examples), Some(with_examples)) = &(
                self.details.total_items_needing_examples,
                self.details.items_with_examples,
            ) {
                ::std::write!(
writer,
"<span class=\"documented-info\"><b>{expr3}</b> out of <b>{expr4}</b> items with examples</span>",
expr3 = &::rinja::MarkupDisplay::new_unsafe(&(with_examples), ::rinja::Html),expr4 = &::rinja::MarkupDisplay::new_unsafe(&(needing_examples), ::rinja::Html),
)?;
            }
            writer.write_str("</li>")?;
        }
        writer
            .write_str("<li class=\"pure-menu-heading\">Links</li>\n\n                        ")?;

        {
            let _iter = (&self.details.dependencies).into_iter();
            for (dep, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n                                            <a href=\"/crate/{expr21}/{expr22}\" class=\"pure-menu-link\">\n                                                {expr21} {expr22}\n                                                ",
expr21 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[0]), ::rinja::Html),expr22 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[1]), ::rinja::Html),
)?;
                if *(&(dep.len() > 2) as &bool) {
                    ::std::write!(
writer,
"\n                                                    <i class=\"dependencies {expr23}\">{expr23}</i>\n                                                    ",
expr23 = &::rinja::MarkupDisplay::new_unsafe(&(&dep[2]), ::rinja::Html),
)?;
                    if *(&(dep.len() > 3) as &bool) {
                        writer.write_str("\n                                                        <i>optional</i>\n                                                    ")?;
                    }
                    writer.write_str("\n                                                ")?;
                } else {
                    writer.write_str("\n                                                    <i class=\"dependencies\"></i>\n                                                ")?;
                }
                writer.write_str("\n                                            </a>\n                                        </li>")?;
            }
        }
        if *(&(self.details.dependencies.is_empty()) as &bool) {
            writer.write_str("&mdash;")?;
        }
        writer.write_str("</ul>\n                            </div>\n                        </li>\n\n                        <li class=\"pure-menu-heading\">Versions</li>\n                        <li class=\"pure-menu-item\">\n                            <div class=\"pure-menu pure-menu-scrollable sub-menu\">\n                                <ul class=\"pure-menu-list\">\n                                    \n                                    ")?;
        {
            let (name, releases, target, inner_path) =
                ((&self.details.name), (&self.details.releases), (&""), (&""));
            {
                let _iter = (releases).into_iter();
                for (release, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                    let release_url = String::new();
                    let retain_fragment = false;
                    writer.write_str("\n        \n        ")?;
                    if *(&(inner_path.is_empty()) as &bool) {
                        writer.write_str(" ")?;
                        let release_url =
                            ::std::format!("/crate/{}/{}", &(name), &(release.version));
                    } else {
                        let release_url = ::std::format!(
                            "/crate/{}/{}/target-redirect/{}{}",
                            &(name),
                            &(release.version),
                            &(target),
                            &(inner_path)
                        );
                        let retain_fragment = true;
                    }
                    let release_name = ::std::format!("{}-{}", &(name), &(release.version));
                    let warning = String::new();
                    writer.write_str("\n        ")?;
                    if *(&(!release.is_library) as &bool) {
                        let warning = ::std::format!("{} is not a library", &(release_name));
                    } else if *(&(release.yanked && release.build_status == "success") as &bool) {
                        let warning = ::std::format!("{} is yanked", &(release_name));
                    } else if *(&(release.yanked && release.build_status == "failure") as &bool) {
                        let warning = ::std::format!(
                            "{} is yanked and docs.rs failed to build it",
                            &(release_name)
                        );
                    } else if *(&(release.build_status == "failure") as &bool) {
                        let warning = ::std::format!("docs.rs failed to build {}", &(release_name));
                    } else if *(&(release.build_status == "in_progress") as &bool) {
                        let title = ::std::format!("{} is currently being built", &(release_name));
                    } else {
                        let warning = false;
                    }
                    if *(&(warning) as &bool) {
                        let title = warning;
                    }
                    ::std::write!(
writer,
"<li class=\"pure-menu-item\">\n            <a\n                href=\"{expr24}\"\n                \n                rel=\"nofollow\"\n                class=\"pure-menu-link",
expr24 = &::rinja::filters::safe(::rinja::Html, &(release_url))?,
)?;
                    if *(&(!warning.is_empty()) as &bool) {
                        writer.write_str(" warn")?;
                    }
                    writer.write_str("\"\n                ")?;
                    if let Some(title) = &self.title {
                        ::std::write!(
                            writer,
                            " title=\"{expr25}\"",
                            expr25 = &::rinja::MarkupDisplay::new_unsafe(&(title), ::rinja::Html),
                        )?;
                    }
                    writer.write_str("\n                ")?;
                    if *(&(retain_fragment) as &bool) {
                        writer.write_str("data-fragment=\"retain\"")?;
                    }
                    writer.write_str("\n            >\n                ")?;
                    if *(&(!warning.is_empty()) as &bool) {
                        ::std::write!(
                            writer,
                            "\n                    {expr26}\n                ",
                            expr26 = &::rinja::MarkupDisplay::new_unsafe(
                                &(filters::fas("triangle-exclamation", false, false, "")?),
                                ::rinja::Html
                            ),
                        )?;
                    }
                    writer.write_str("\n                ")?;
                    if *(&(release.build_status == "in_progress") as &bool) {
                        ::std::write!(
                            writer,
                            "\n                    {expr27}\n                ",
                            expr27 = &::rinja::MarkupDisplay::new_unsafe(
                                &(filters::fas("gear", true, true, "")?),
                                ::rinja::Html
                            ),
                        )?;
                    }
                    ::std::write!(
                        writer,
                        "\n                {expr28}\n            </a>\n        </li>",
                        expr28 =
                            &::rinja::MarkupDisplay::new_unsafe(&(release.version), ::rinja::Html),
                    )?;
                }
            }
        }
        writer.write_str("\n                                </ul>\n                            </div>\n                        </li>\n\n                        \n                        <li class=\"pure-menu-heading\">Owners</li>\n                        <li class=\"pure-menu-item\">")?;
        {
            let _iter = (&self.details.owners).into_iter();
            for (owner, _loop_item) in ::rinja::helpers::TemplateLoop::new(_iter) {
                ::std::write!(
writer,
"<a href=\"https://crates.io/users/{expr29}\">\n                                    <img src=\"{expr30}\" alt=\"{expr29}\" class=\"owner\">\n                                </a>",
expr29 = &::rinja::MarkupDisplay::new_unsafe(&(&owner[0]), ::rinja::Html),expr30 = &::rinja::MarkupDisplay::new_unsafe(&(&owner[1]), ::rinja::Html),
)?;
            }
        }
        writer.write_str("</li>\n                    </ul>\n                </div>\n            </div>\n\n            <div class=\"pure-u-1 pure-u-sm-17-24 pure-u-md-19-24 package-details\" id=\"main\">\n                ")?;
        if *(&(!self.details.is_library) as &bool) {
            ::std::write!(
writer,
"<div class=\"warning\">\n                        {expr31}-{expr32} is not a library.\n                    </div>\n\n                ",
expr31 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr32 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.version), ::rinja::Html),
)?;
        } else if *(&(self.details.metadata.yanked.unwrap_or_default()) as &bool) {
            ::std::write!(
writer,
"<div class=\"warning\">\n                        {expr33}-{expr34} has been yanked.\n                    </div>\n\n                ",
expr33 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr34 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.version), ::rinja::Html),
)?;
        } else if *(&(self.details.build_status == "success") as &bool) {
            if *(&(!self.details.rustdoc_status) as &bool) {
                ::std::write!(
writer,
"<div class=\"warning\">{expr35}-{expr36} doesn't have any documentation.</div>",
expr35 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr36 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.version), ::rinja::Html),
)?;
            }
        } else if *(&(self.details.build_status == "failure") as &bool) {
            ::std::write!(
writer,
"\n                    <div class=\"warning\">\n                        docs.rs failed to build {expr37}-{expr38}\n                        <br>\n                        Please check the\n                        <a href=\"/crate/{expr37}/{expr38}/builds\">build logs</a> for more information.\n                        <br>\n                        See <a href=\"/about/builds\">Builds</a> for ideas on how to fix a failed build,\n                        or <a href=\"/about/metadata\">Metadata</a> for how to configure docs.rs builds.\n                        <br>\n                        If you believe this is docs.rs' fault, <a href=\"https://github.com/rust-lang/docs.rs/issues/new/choose\">open an issue</a>.\n                    </div>",
expr37 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr38 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.version), ::rinja::Html),
)?;
        } else if *(&(self.details.build_status == "in_progress") as &bool) {
            ::std::write!(
writer,
"<div class=\"info\">\n                        {expr39}\n                        Build is in progress, it will be available soon\n                    </div>",
expr39 = &::rinja::MarkupDisplay::new_unsafe(&(filters::fas("gear", false, true, "")?), ::rinja::Html),
)?;
        }
        if *(&(self.details.last_successful_build) as &bool) {
            ::std::write!(
writer,
"<div class=\"info\">\n                        Visit the last successful build:\n                        <a href=\"/crate/{expr40}/{expr41}\">\n                            {expr40}-{expr41}\n                        </a>\n                    </div>",
expr40 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.name), ::rinja::Html),expr41 = &::rinja::MarkupDisplay::new_unsafe(&(self.details.last_successful_build), ::rinja::Html),
)?;
        }
        if *(&(self.details.readme) as &bool) {
            ::std::write!(
                writer,
                "{expr42}\n\n                ",
                expr42 = &::rinja::filters::safe(::rinja::Html, &(self.details.readme))?,
            )?;
        } else if *(&(self.details.rustdoc) as &bool) {
            ::std::write!(
                writer,
                "{expr43}",
                expr43 = &::rinja::filters::safe(::rinja::Html, &(self.details.rustdoc))?,
            )?;
        }
        writer.write_str("</div>\n        </div>\n    </div>")?;
        writer.write_str("</body>\n</html>")?;
        ::rinja::Result::Ok(())
    }
    const EXTENSION: ::std::option::Option<&'static ::std::primitive::str> = Some("html");
    const SIZE_HINT: ::std::primitive::usize = 14844;
    const MIME_TYPE: &'static ::std::primitive::str = "text/html; charset=utf-8";
}
impl ::std::fmt::Display for CrateDetailsPage {
    #[inline]
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        ::rinja::Template::render_into(self, f).map_err(|_| ::std::fmt::Error {})
    }
}
