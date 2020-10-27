use crate::{
    db::Pool,
    impl_webpage,
    web::{page::WebPage, MetaData},
};
use iron::{IronResult, Request, Response};
use router::Router;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct FeaturesPage {
    metadata: MetaData,
}

impl_webpage! {
    FeaturesPage = "crate/features.html",
}

pub fn build_features_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(req, router.find("name"));
    let version = cexpect!(req, router.find("version"));

    let mut conn = extension!(req, Pool).get()?;

    FeaturesPage {
        metadata: cexpect!(req, MetaData::from_crate(&mut conn, &name, &version)),
    }
    .into_response(req)
}
