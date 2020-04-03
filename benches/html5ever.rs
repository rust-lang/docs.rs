use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cratesfyi::utils::extract_head_and_body;

pub fn criterion_benchmark(c: &mut Criterion) {
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
    c.bench_function("parse regex html", |b| {
        b.iter(|| extract_head_and_body(black_box(&html)))
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
