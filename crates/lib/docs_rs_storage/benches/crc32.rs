use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use docs_rs_storage::crc32_for_path;
use std::{fs, hint::black_box};

pub fn crc32_file(c: &mut Criterion) {
    let fixture_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();

    let fixture = vec![b'x'; 16 * 1024 * 1024];
    fs::write(&fixture_path, &fixture).unwrap();

    let mut group = c.benchmark_group("crc32");
    group.throughput(Throughput::Bytes(fixture.len() as u64));
    group.bench_function("file_16mib", |b| {
        b.iter(|| crc32_for_path(black_box(&fixture_path)).unwrap());
    });
    group.finish();
}

criterion_group!(crc32_benches, crc32_file);
criterion_main!(crc32_benches);
