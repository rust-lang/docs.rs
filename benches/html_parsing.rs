use cratesfyi::utils::extract_head_and_body;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

pub fn regex_capture_matches(c: &mut Criterion) {
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();

    c.benchmark_group("regex html")
        .throughput(Throughput::Bytes(html.as_bytes().len() as u64))
        .bench_function("extract head and body", |b| {
            b.iter(|| extract_head_and_body(black_box(&html)))
        });
}

criterion_group!(html_parsing, regex_capture_matches);
criterion_main!(html_parsing);
