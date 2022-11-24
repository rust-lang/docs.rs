use super::TemplateData;
use crate::{
    ctry,
    utils::spawn_blocking,
    web::{cache::CachePolicy, csp::Csp, error::AxumNope},
};
use anyhow::Error;
use axum::{
    body::{boxed, Body},
    http::Request as AxumRequest,
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
};
use futures_util::future::{BoxFuture, FutureExt};
use http::header::CONTENT_LENGTH;
use iron::{
    headers::{ContentType, Link, LinkValue, RelationType},
    response::Response,
    status::Status,
    IronResult, Request,
};
use serde::Serialize;
use std::{borrow::Cow, sync::Arc};
use tera::Context;

/// When making using a custom status, use a closure that coerces to a `fn(&Self) -> Status`
#[macro_export]
macro_rules! impl_webpage {
    ($page:ty = $template:literal $(, status = $status:expr)? $(, content_type = $content_type:expr)?  $(, canonical_url = $canonical_url:expr)? $(,)?) => {
        $crate::impl_webpage!($page = |_| ::std::borrow::Cow::Borrowed($template) $(, status = $status)? $(, content_type = $content_type)?  $(, canonical_url = $canonical_url)?);
    };

    ($page:ty = $template:expr $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(, canonical_url = $canonical_url:expr)? $(,)?) => {
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
                fn canonical_url(&self) -> Option<String> {
                    let canonical_url: fn(&Self) -> Option<String> = $canonical_url;
                    (canonical_url)(self)
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
    (
        $page:ty = $template:literal
        $(, status = $status:expr)?
        $(, content_type = $content_type:expr)?
        $(, cpu_intensive_rendering = $cpu_intensive_rendering:expr)?
        $(,)?
    ) => {
        $crate::impl_axum_webpage!(
            $page = |_| ::std::borrow::Cow::Borrowed($template)
            $(, status = $status)?
            $(, content_type = $content_type)?
            $(, cpu_intensive_rendering = $cpu_intensive_rendering )?
         );
    };

    (
        $page:ty = $template:expr
        $(, status = $status:expr)?
        $(, content_type = $content_type:expr)?
        $(, cpu_intensive_rendering = $cpu_intensive_rendering:expr)?
        $(,)?
    ) => {
        impl axum::response::IntoResponse for $page
        {
            fn into_response(self) -> ::axum::response::Response {
                // set a default content type, eventually override from the page
                #[allow(unused_mut, unused_assignments)]
                let mut ct: &'static str = ::mime::TEXT_HTML_UTF_8.as_ref();
                $(
                    ct = $content_type;
                )?

                #[allow(unused_mut, unused_assignments)]
                let mut cpu_intensive_rendering = false;
                $(
                    cpu_intensive_rendering = $cpu_intensive_rendering;
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
                    cpu_intensive_rendering,
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

        if let Some(canonical_url) = self.canonical_url() {
            let link_value = LinkValue::new(canonical_url)
                .push_rel(RelationType::ExtRelType("canonical".to_string()));

            response.headers.set(Link::new(vec![link_value]));
        }

        Ok(response)
    }

    /// The name of the template to be rendered
    fn template(&self) -> Cow<'static, str>;

    /// The canonical URL to set in response headers
    fn canonical_url(&self) -> Option<String> {
        None
    }

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
    pub cpu_intensive_rendering: bool,
}

fn render_response(
    mut response: AxumResponse,
    templates: Arc<TemplateData>,
    csp_nonce: String,
) -> BoxFuture<'static, AxumResponse> {
    async move {
        if let Some(render) = response.extensions_mut().remove::<DelayedTemplateRender>() {
            let DelayedTemplateRender {
                template,
                mut context,
                cpu_intensive_rendering,
            } = render;
            context.insert("csp_nonce", &csp_nonce);

            let rendered = if cpu_intensive_rendering {
                spawn_blocking({
                    let templates = templates.clone();
                    move || Ok(templates.templates.render(&template, &context)?)
                })
                .await
            } else {
                templates
                    .templates
                    .render(&template, &context)
                    .map_err(Error::new)
            };

            let rendered = match rendered {
                Ok(content) => content,
                Err(err) => {
                    if response.status().is_server_error() {
                        // avoid infinite loop if error.html somehow fails to load
                        panic!("error while serving error page: {:?}", err);
                    } else {
                        return render_response(
                            AxumNope::InternalError(err).into_response(),
                            templates,
                            csp_nonce,
                        )
                        .await;
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
    .boxed()
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

    let response = next.run(req).await;

    render_response(response, templates, csp_nonce).await
}
