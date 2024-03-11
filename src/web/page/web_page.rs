use super::TemplateData;
use crate::web::{csp::Csp, error::AxumNope};
use axum::{
    body::Body,
    extract::Request as AxumRequest,
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
};
use futures_util::future::{BoxFuture, FutureExt};
use http::header::CONTENT_LENGTH;
use std::sync::Arc;

pub(crate) trait AddCspNonce: IntoResponse {
    fn set_csp_nonce(&mut self, csp_nonce: String);
}

#[macro_export]
macro_rules! impl_axum_webpage {
    (
        $page:ty
        $(, status = $status:expr)?
        $(, content_type = $content_type:expr)?
        $(, canonical_url = $canonical_url:expr)?
        $(, cache_policy  = $cache_policy:expr)?
        $(, cpu_intensive_rendering = $cpu_intensive_rendering:expr)?
        $(,)?
    ) => {
        impl crate::web::page::web_page::AddCspNonce for $page {
            fn set_csp_nonce(&mut self, csp_nonce: String) {
                self.csp_nonce = csp_nonce;
            }
        }

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
                    template: Box::new(self),
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
    pub template: Box<dyn AddCspNonce + Sync + Send>,
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
                mut template,
                cpu_intensive_rendering,
            } = render;
            template.set_csp_nonce(csp_nonce.clone());

            let rendered = if cpu_intensive_rendering {
                templates.render_in_threadpool(move || Ok(template.into_response())).await
            } else {
                template.into_response()
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

pub(crate) async fn render_templates_middleware(req: AxumRequest, next: Next) -> AxumResponse {
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
