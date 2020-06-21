use crate::config::Config;
use crate::web::page::TemplateData;
use crate::web::Pool;
use iron::{BeforeMiddleware, IronResult, Request};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct InjectExtensions {
    pub(crate) pool: Pool,
    pub(crate) config: Arc<Config>,
    pub(crate) template_data: Arc<TemplateData>,
}

impl BeforeMiddleware for InjectExtensions {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<Pool>(self.pool.clone());
        req.extensions.insert::<Config>(self.config.clone());
        req.extensions
            .insert::<TemplateData>(self.template_data.clone());

        Ok(())
    }
}

macro_rules! key {
    ($key:ty => $value:ty) => {
        impl iron::typemap::Key for $key {
            type Value = $value;
        }
    };
}

key!(Pool => Pool);
key!(Config => Arc<Config>);
key!(TemplateData => Arc<TemplateData>);
