use cratesfyi::storage::{compress, decompress, CompressionAlgorithm};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

const ALGORITHM: CompressionAlgorithm = CompressionAlgorithm::Zstd;

pub fn regex_capture_matches(c: &mut Criterion) {
    // this isn't a great benchmark because it only tests on one file
    // ideally we would build a whole crate and compress each file, taking the average
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
    let html_slice = html.as_bytes();

    c.benchmark_group("regex html")
        .throughput(Throughput::Bytes(html_slice.len() as u64))
        .bench_function("compress", |b| {
            b.iter(|| compress(black_box(html_slice), ALGORITHM));
        })
        .bench_function("decompress", |b| {
            b.iter(|| decompress(black_box(html_slice), ALGORITHM, 5 * 1024 * 1024));
        });
}

criterion_group!(compression, regex_capture_matches);
criterion_main!(compression);
