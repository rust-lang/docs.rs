pub(crate) mod templates;
pub(crate) mod web_page;

pub(crate) use templates::TemplateData;

use crate::f_a::IconStr;
use serde::ser::{Serialize, SerializeStruct, Serializer};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct GlobalAlert {
    pub(crate) url: &'static str,
    pub(crate) text: &'static str,
    pub(crate) css_class: &'static str,
    pub(crate) fa_icon: crate::f_a::icons::IconTriangleExclamation,
}

impl Serialize for GlobalAlert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("GlobalAlert", 4)?;
        s.serialize_field("url", &self.url)?;
        s.serialize_field("text", &self.text)?;
        s.serialize_field("css_class", &self.css_class)?;
        s.serialize_field("fa_icon", &self.fa_icon.icon_name())?;
        s.end()
    }
}

#[cfg(test)]
mod tera_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialize_global_alert() {
        let alert = GlobalAlert {
            url: "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            text: "THE WORLD WILL SOON END",
            css_class: "THE END IS NEAR",
            fa_icon: crate::f_a::icons::IconTriangleExclamation,
        };

        let correct_json = json!({
            "url": "http://www.hasthelargehadroncolliderdestroyedtheworldyet.com/",
            "text": "THE WORLD WILL SOON END",
            "css_class": "THE END IS NEAR",
            "fa_icon": "triangle-exclamation"
        });

        assert_eq!(correct_json, serde_json::to_value(alert).unwrap());
    }
}
