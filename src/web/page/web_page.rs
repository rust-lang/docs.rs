use super::TemplateData;
use crate::{
    config::PublicConfig,
    web::{csp::Csp, error::AxumNope},
    Config,
};
use anyhow::Error;
use axum::{
    body::Body,
    extract::Request as AxumRequest,
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
    Extension,
};
use futures_util::future::{BoxFuture, FutureExt};
use http::header::CONTENT_LENGTH;
use std::sync::Arc;
use tera::Context;

#[macro_export]
macro_rules! impl_axum_webpage {
    (
        $page:ty = $template:literal
        $(, status = $status:expr)?
        $(, content_type = $content_type:expr)?
        $(, canonical_url = $canonical_url:expr)?
        $(, cache_policy  = $cache_policy:expr)?
        $(, cpu_intensive_rendering = $cpu_intensive_rendering:expr)?
        $(,)?
    ) => {
        $crate::impl_axum_webpage!(
            $page = |_| ::std::borrow::Cow::Borrowed($template)
            $(, status = $status)?
            $(, content_type = $content_type)?
            $(, canonical_url = $canonical_url)?
            $(, cache_policy = $cache_policy)?
            $(, cpu_intensive_rendering = $cpu_intensive_rendering )?
         );
    };

    (
        $page:ty = $template:expr
        $(, status = $status:expr)?
        $(, content_type = $content_type:expr)?
        $(, canonical_url = $canonical_url:expr)?
        $(, cache_policy  = $cache_policy:expr)?
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
                    .body(::axum::body::Body::empty())
                    .unwrap();

                $(
                    response.extensions_mut().insert({
                        let cache_policy: fn(&$page) -> $crate::web::cache::CachePolicy = $cache_policy;
                        (cache_policy)(&self)
                    });
                )?

                $(
                    let canonical_url = {
                        let canonical_url: fn(&Self) -> Option<$crate::web::headers::CanonicalUrl> = $canonical_url;
                        (canonical_url)(&self)
                    };
                    if let Some(canonical_url) = canonical_url {
                        use axum_extra::headers::HeaderMapExt;

                        response.headers_mut().typed_insert(canonical_url);
                    }
                )?


                response.extensions_mut().insert($crate::web::page::web_page::DelayedTemplateRender {
                    context: {
                        let mut c = ::tera::Context::from_serialize(&self)
                            .expect("could not create tera context from web-page");
                        c.insert("DEFAULT_MAX_TARGETS", &$crate::DEFAULT_MAX_TARGETS);
                        c
                    },
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

/// adding this to the axum response extensions will lead
/// to the template being rendered, adding the csp_nonce to
/// the context.
#[derive(Clone)]
pub(crate) struct DelayedTemplateRender {
    pub template: String,
    pub context: Context,
    pub cpu_intensive_rendering: bool,
}

fn render_response(
    mut response: AxumResponse,
    templates: Arc<TemplateData>,
    config: Arc<Config>,
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
            context.insert("config", &PublicConfig(config.clone()));

            let rendered = if cpu_intensive_rendering {
                templates
                    .render_in_threadpool(move |templates| {
                        templates
                            .templates
                            .render(&template, &context)
                            .map_err(Into::into)
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
                        panic!("error while serving error page: {err:?}");
                    } else {
                        return render_response(
                            AxumNope::InternalError(err).into_response(),
                            templates,
                            config,
                            csp_nonce,
                        )
                        .await;
                    }
                }
            };
            let content_length = rendered.len();
            *response.body_mut() = Body::from(rendered);
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

pub(crate) async fn render_templates_middleware(
    Extension(config): Extension<Arc<Config>>,
    Extension(templates): Extension<Arc<TemplateData>>,
    Extension(csp): Extension<Arc<Csp>>,
    req: AxumRequest,
    next: Next,
) -> AxumResponse {
    let csp_nonce = csp.nonce().to_owned();

    let response = next.run(req).await;

    render_response(response, templates, config, csp_nonce).await
}
