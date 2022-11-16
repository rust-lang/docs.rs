use super::TemplateData;
use crate::{
    ctry,
    web::{cache::CachePolicy, csp::Csp, error::AxumNope},
};
use anyhow::anyhow;
use axum::{
    body::{boxed, Body},
    http::Request as AxumRequest,
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
};
use http::header::CONTENT_LENGTH;
use iron::{headers::ContentType, response::Response, status::Status, IronResult, Request};
use serde::Serialize;
use std::{borrow::Cow, sync::Arc};
use tera::{Context, Tera};

/// When making using a custom status, use a closure that coerces to a `fn(&Self) -> Status`
#[macro_export]
macro_rules! impl_webpage {
    ($page:ty = $template:literal $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(,)?) => {
        $crate::impl_webpage!($page = |_| ::std::borrow::Cow::Borrowed($template) $(, status = $status)? $(, content_type = $content_type)?);
    };

    ($page:ty = $template:expr $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(,)?) => {
        impl $crate::web::page::WebPage for $page {
            fn template(&self) -> ::std::borrow::Cow<'static, str> {
                let template: fn(&Self) -> ::std::borrow::Cow<'static, str> = $template;
                template(self)
            }

            $(
                fn get_status(&self) -> ::iron::status::Status {
                    let status: fn(&Self) -> ::iron::status::Status = $status;
                    (status)(self)
                }
            )?

            $(
                fn content_type() -> ::iron::headers::ContentType {
                    $content_type
                }
            )?
        }
    };
}

#[macro_export]
macro_rules! impl_axum_webpage {
    ($page:ty = $template:literal $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(,)?) => {
        $crate::impl_axum_webpage!($page = |_| ::std::borrow::Cow::Borrowed($template) $(, status = $status)? $(, content_type = $content_type)?);
    };

    ($page:ty = $template:expr $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(,)?) => {
        impl axum::response::IntoResponse for $page
        {
            fn into_response(self) -> ::axum::response::Response {
                // set a default content type, eventually override from the page
                #[allow(unused_mut, unused_assignments)]
                let mut ct: &'static str = ::mime::TEXT_HTML_UTF_8.as_ref();
                $(
                    ct = $content_type;
                )?

                let mut response = ::axum::http::Response::builder()
                    .header(::axum::http::header::CONTENT_TYPE, ct)
                    $(
                        .status({
                            let status: fn(&$page) -> ::axum::http::StatusCode = $status;
                            (status)(&self)
                        })
                    )?
                    // this empty body will be replaced in `render_templates_middleware` using
                    // the data from `DelayedTemplateRender` below.
                    .body(::axum::body::boxed(::axum::body::Body::empty()))
                    .unwrap();

                response.extensions_mut().insert($crate::web::page::web_page::DelayedTemplateRender {
                    context: ::tera::Context::from_serialize(&self)
                        .expect("could not create tera context from web-page"),
                    template: {
                        let template: fn(&Self) -> ::std::borrow::Cow<'static, str> = $template;
                        template(&self).to_string()
                    },
                });
                response
            }
        }
    };
}

#[derive(Serialize)]
struct TemplateContext<'a, T> {
    csp_nonce: &'a str,
    #[serde(flatten)]
    page: &'a T,
}

/// The central trait that rendering pages revolves around, it handles selecting and rendering the template
pub trait WebPage: Serialize + Sized {
    /// Turn the current instance into a `Response`, ready to be served
    // TODO: We could cache similar pages using the `&Context`
    fn into_response(self, req: &Request) -> IronResult<Response> {
        let csp_nonce = req
            .extensions
            .get::<Csp>()
            .expect("missing CSP from the request extensions")
            .nonce();

        let ctx = Context::from_serialize(&TemplateContext {
            csp_nonce,
            page: &self,
        })
        .unwrap();
        let status = self.get_status();
        let result = req
            .extensions
            .get::<TemplateData>()
            .expect("missing TemplateData from the request extensions")
            .templates
            .render(&self.template(), &ctx);

        let rendered = if status.is_server_error() {
            // avoid infinite loop if error.html somehow fails to load
            result.expect("error while serving error page")
        } else {
            ctry!(req, result)
        };

        let mut response = Response::with((status, rendered));
        response.headers.set(Self::content_type());
        if let Some(cache) = Self::cache_policy() {
            response.extensions.insert::<CachePolicy>(cache);
        }

        Ok(response)
    }

    /// The name of the template to be rendered
    fn template(&self) -> Cow<'static, str>;

    /// Gets the status of the request, defaults to `Ok`
    fn get_status(&self) -> Status {
        Status::Ok
    }

    /// The content type that the template should be served with, defaults to html
    fn content_type() -> ContentType {
        ContentType::html()
    }

    /// caching for this page.
    /// `None` leads to the default from the `CacheMiddleware`
    /// being used.
    fn cache_policy() -> Option<CachePolicy> {
        None
    }
}

/// adding this to the axum response extensions will lead
/// to the template being rendered, adding the csp_nonce to
/// the context.
pub(crate) struct DelayedTemplateRender {
    pub template: String,
    pub context: Context,
}

fn render_response(mut response: AxumResponse, templates: &Tera, csp_nonce: &str) -> AxumResponse {
    if let Some(render) = response.extensions().get::<DelayedTemplateRender>() {
        let mut context = render.context.clone();
        context.insert("csp_nonce", &csp_nonce);

        let rendered = match templates.render(&render.template, &context) {
            Ok(content) => content,
            Err(err) => {
                if response.status().is_server_error() {
                    // avoid infinite loop if error.html somehow fails to load
                    panic!("error while serving error page: {:?}", err);
                } else {
                    return render_response(
                        AxumNope::InternalError(anyhow!(err)).into_response(),
                        templates,
                        csp_nonce,
                    );
                }
            }
        };
        let content_length = rendered.len();
        *response.body_mut() = boxed(Body::from(rendered));
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, content_length.into());
        response
    } else {
        response
    }
}

pub(crate) async fn render_templates_middleware<B>(
    req: AxumRequest<B>,
    next: Next<B>,
) -> AxumResponse {
    let templates: Arc<TemplateData> = req
        .extensions()
        .get::<Arc<TemplateData>>()
        .expect("template data request extension not found")
        .clone();

    let csp_nonce = req
        .extensions()
        .get::<Arc<Csp>>()
        .expect("csp request extension not found")
        .nonce()
        .to_owned();

    render_response(next.run(req).await, &templates.templates, &csp_nonce)
}
