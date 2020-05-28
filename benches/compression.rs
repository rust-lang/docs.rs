use cratesfyi::storage::{compress, decompress};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

pub fn criterion_benchmark(c: &mut Criterion) {
    // this isn't a great benchmark because it only tests on one file
    // ideally we would build a whole crate and compress each file, taking the average
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
    let html_slice = html.as_bytes();
    c.bench_function("compress regex html", |b| {
        b.iter(|| compress(black_box(html_slice)))
    });
    let (compressed, alg) = compress(html_slice).unwrap();
    c.bench_function("decompress regex html", |b| {
        b.iter(|| decompress(black_box(compressed.as_slice()), alg))
    });
}

criterion_group!(compression, criterion_benchmark);
criterion_main!(compression);
