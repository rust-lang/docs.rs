use super::TemplateData;
use iron::{headers::ContentType, response::Response, status::Status, IronResult, Request};
use serde::Serialize;
use tera::Context;

/// When making using a custom status, use a closure that coerces to a `fn(&Self) -> Status`
#[macro_export]
macro_rules! impl_webpage {
    ($page:ty = $template:expr $(, status = $status:expr)? $(, content_type = $content_type:expr)? $(,)?) => {
        impl $crate::web::page::WebPage for $page {
            const TEMPLATE: &'static str = $template;

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

pub(in crate::web) fn respond(
    template: &str,
    ctx: Context,
    content_type: ContentType,
    status: Status,
    req: &Request,
) -> IronResult<Response> {
    let rendered = req
        .extensions
        .get::<TemplateData>()
        .expect("missing TemplateData from the request extensions")
        .templates
        .load()
        .render(template, &ctx)
        .unwrap();

    let mut response = Response::with((status, rendered));
    response.headers.set(content_type);

    Ok(response)
}

/// The central trait that rendering pages revolves around, it handles selecting and rendering the template
pub trait WebPage: Serialize + Sized {
    /// Turn the current instance into a `Response`, ready to be served
    // TODO: We could cache similar pages using the `&Context`
    fn into_response(self, req: &Request) -> IronResult<Response> {
        let ctx = Context::from_serialize(&self).unwrap();
        respond(
            Self::TEMPLATE,
            ctx,
            Self::content_type(),
            self.get_status(),
            req,
        )
    }

    /// The name of the template to be rendered
    const TEMPLATE: &'static str;

    /// Gets the status of the request, defaults to `Ok`
    fn get_status(&self) -> Status {
        Status::Ok
    }

    /// The content type that the template should be served with, defaults to html
    fn content_type() -> ContentType {
        ContentType::html()
    }
}
