use time::Timespec;

pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: String,
    pub(crate) date_updated: Timespec,
    pub(crate) content: Vec<u8>,
}
