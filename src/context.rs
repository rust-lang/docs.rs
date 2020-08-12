use crate::db::Pool;
use crate::{BuildQueue, Config, Metrics, Storage};
use failure::Error;
use std::sync::Arc;

pub trait Context {
    fn config(&self) -> Result<Arc<Config>, Error>;
    fn build_queue(&self) -> Result<Arc<BuildQueue>, Error>;
    fn storage(&self) -> Result<Arc<Storage>, Error>;
    fn pool(&self) -> Result<Pool, Error>;
    fn metrics(&self) -> Result<Arc<Metrics>, Error>;
}
