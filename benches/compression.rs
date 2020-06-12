use cratesfyi::storage::{compress, decompress, CompressionAlgorithm};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const ALGORITHM: CompressionAlgorithm = CompressionAlgorithm::Zstd;

pub fn criterion_benchmark(c: &mut Criterion) {
    // this isn't a great benchmark because it only tests on one file
    // ideally we would build a whole crate and compress each file, taking the average
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
    let html_slice = html.as_bytes();
    c.bench_function("compress regex html", |b| {
        b.iter(|| compress(black_box(html_slice, ALGORITHM)))
    });
    let compressed = compress(html_slice, ALGORITHM).unwrap();
    c.bench_function("decompress regex html", |b| {
        b.iter(|| decompress(black_box(compressed.as_slice()), ALGORITHM))
    });
}

criterion_group!(compression, criterion_benchmark);
criterion_main!(compression);
