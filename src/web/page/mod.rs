pub(crate) mod templates;
pub(crate) mod web_page;

pub(crate) use templates::TemplateData;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct GlobalAlert {
    pub(crate) url: &'static str,
    pub(crate) text: &'static str,
    pub(crate) css_class: &'static str,
    pub(crate) fa_icon: crate::f_a::icons::IconTriangleExclamation,
}
