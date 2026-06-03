use crate::types::FileRange;
use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use strum::IntoStaticStr;
use tracing::Span;

const KIB: f64 = 1024.0;
const MIB: f64 = 1024.0 * KIB;
const GIB: f64 = 1024.0 * MIB;

/// boundaries for histogram metrics where we track
/// file sizes on our S3 bucket.
/// This has to include:
/// * zip archives (between 500 KiB & 10 GiB)
/// * archive indexes (between <100 KiB & 500 MiB)
/// * plain old html / css files (mostly super small)
pub(crate) const FILE_SIZE_HISTOGRAM_BUCKETS: &[f64] = &[
    KIB,
    4.0 * KIB,
    16.0 * KIB,
    64.0 * KIB,
    256.0 * KIB,
    512.0 * KIB,
    MIB,
    2.0 * MIB,
    4.0 * MIB,
    8.0 * MIB,
    16.0 * MIB,
    32.0 * MIB,
    64.0 * MIB,
    128.0 * MIB,
    256.0 * MIB,
    512.0 * MIB,
    GIB,
    2.0 * GIB,
    4.0 * GIB,
    8.0 * GIB,
    10.0 * GIB,
];

#[derive(Copy, Clone, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum UploadType {
    Single,
    MultiPart,
}

#[derive(Debug)]
pub(crate) struct StorageMetrics {
    pub(crate) exist_calls: Counter<u64>,
    pub(crate) uploaded_files: Counter<u64>,
    pub(crate) uploaded_bytes: Counter<u64>,
    pub(crate) uploaded_entry_size: Histogram<u64>,
    pub(crate) downloaded_files: Counter<u64>,
    pub(crate) downloaded_bytes: Counter<u64>,
    pub(crate) downloaded_entry_size: Histogram<u64>,
    pub(crate) deleted_files: Counter<u64>,
}

impl StorageMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("storage");
        const PREFIX: &str = "docsrs.storage";

        Self {
            exist_calls: meter
                .u64_counter(format!("{PREFIX}.exist_calls"))
                .with_unit("1")
                .build(),
            uploaded_files: meter
                .u64_counter(format!("{PREFIX}.uploaded_files"))
                .with_unit("1")
                .build(),
            uploaded_bytes: meter
                .u64_counter(format!("{PREFIX}.uploaded_bytes"))
                .with_unit("By")
                .build(),
            uploaded_entry_size: meter
                .u64_histogram(format!("{PREFIX}.uploaded_entry_size"))
                .with_unit("By")
                .with_boundaries(FILE_SIZE_HISTOGRAM_BUCKETS.to_vec())
                .build(),
            downloaded_files: meter
                .u64_counter(format!("{PREFIX}.downloaded_files"))
                .with_unit("1")
                .build(),
            downloaded_bytes: meter
                .u64_counter(format!("{PREFIX}.downloaded_bytes"))
                .with_unit("By")
                .build(),
            downloaded_entry_size: meter
                .u64_histogram(format!("{PREFIX}.downloaded_entry_size"))
                .with_unit("By")
                .with_boundaries(FILE_SIZE_HISTOGRAM_BUCKETS.to_vec())
                .build(),
            deleted_files: meter
                .u64_counter(format!("{PREFIX}.deleted_files"))
                .with_unit("1")
                .build(),
        }
    }

    pub(crate) fn record_download_metrics(&self, content_length: u64, range: Option<&FileRange>) {
        let download_type = if range.is_some() { "range" } else { "full" };
        Span::current()
            .record("storage.download_type", download_type)
            .record("storage.content_length", content_length);

        let attrs = [KeyValue::new("download_type", download_type)];
        self.downloaded_files.add(1, &attrs);
        self.downloaded_bytes.add(content_length, &attrs);
        self.downloaded_entry_size.record(content_length, &attrs);
    }

    pub(crate) fn record_upload_metrics(&self, content_length: u64, upload_type: UploadType) {
        let upload_type: &str = upload_type.into();

        Span::current()
            .record("storage.upload_type", upload_type)
            .record("storage.content_length", content_length);

        let attrs = [KeyValue::new("upload_type", upload_type)];
        self.uploaded_files.add(1, &attrs);
        self.uploaded_bytes.add(content_length, &attrs);
        self.uploaded_entry_size.record(content_length, &attrs);
    }
}
