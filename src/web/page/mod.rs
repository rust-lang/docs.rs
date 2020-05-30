use serde::Serialize;

mod models;
mod templates;

pub(crate) use models::{
    About, BuildsPage, CrateDetailsPage, Error, HomePage, ReleaseActivity, ReleaseFeed,
    ReleaseQueue, ReleaseType, RustdocPage, Search, SitemapXml, SourcePage, ViewReleases, WebPage,
};
pub(crate) use templates::TemplateData;

lazy_static::lazy_static! {
    /// Holds all data relevant to templating
    pub(crate) static ref TEMPLATE_DATA: TemplateData = TemplateData::new().expect("Failed to load template data");
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GlobalAlert {
    pub(crate) url: &'static str,
    pub(crate) text: &'static str,
    pub(crate) css_class: &'static str,
    pub(crate) fa_icon: &'static str,
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
}
