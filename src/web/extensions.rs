use crate::config::Config;
use crate::db::Pool;
use crate::storage::Storage;
use crate::web::page::TemplateData;
use crate::BuildQueue;
use iron::{BeforeMiddleware, IronResult, Request};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct InjectExtensions {
    pub(super) build_queue: Arc<BuildQueue>,
    pub(super) pool: Pool,
    pub(super) config: Arc<Config>,
    pub(super) storage: Arc<Storage>,
    pub(super) template_data: Arc<TemplateData>,
}

impl BeforeMiddleware for InjectExtensions {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions
            .insert::<BuildQueue>(self.build_queue.clone());
        req.extensions.insert::<Pool>(self.pool.clone());
        req.extensions.insert::<Config>(self.config.clone());
        req.extensions.insert::<Storage>(self.storage.clone());
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

key!(BuildQueue => Arc<BuildQueue>);
key!(Pool => Pool);
key!(Config => Arc<Config>);
key!(Storage => Arc<Storage>);
key!(TemplateData => Arc<TemplateData>);
