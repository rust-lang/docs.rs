use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use docs_rs_storage::{compress, decompress};
use docs_rs_types::CompressionAlgorithm;
use std::hint::black_box;

pub fn regex_capture_matches(c: &mut Criterion) {
    // this isn't a great benchmark because it only tests on one file
    // ideally we would build a whole crate and compress each file, taking the average
    let html = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
    let html = html.repeat(100); // 100 KiB * 100 => ~10 MiB
    let html_slice = html.as_bytes();

    let max_size = html.len() + 1;

    // Pre-compress data for decompression benchmarks
    let compressed_zstd = compress(html_slice, CompressionAlgorithm::Zstd).unwrap();
    let compressed_bzip2 = compress(html_slice, CompressionAlgorithm::Bzip2).unwrap();
    let compressed_gzip = compress(html_slice, CompressionAlgorithm::Gzip).unwrap();
    let compressed_deflate = compress(html_slice, CompressionAlgorithm::Deflate).unwrap();

    c.benchmark_group("regex html")
        .throughput(Throughput::Bytes(html_slice.len() as u64))
        .sample_size(10)
        .bench_function("compress zstd", |b| {
            b.iter(|| compress(black_box(html_slice), CompressionAlgorithm::Zstd));
        })
        .bench_function("decompress zstd", |b| {
            b.iter(|| {
                decompress(
                    black_box(compressed_zstd.as_slice()),
                    CompressionAlgorithm::Zstd,
                    max_size,
                )
            });
        })
        .bench_function("compress bzip2", |b| {
            b.iter(|| compress(black_box(html_slice), CompressionAlgorithm::Bzip2));
        })
        .bench_function("decompress bzip2", |b| {
            b.iter(|| {
                decompress(
                    black_box(compressed_bzip2.as_slice()),
                    CompressionAlgorithm::Bzip2,
                    max_size,
                )
            });
        })
        .bench_function("compress gzip", |b| {
            b.iter(|| compress(black_box(html_slice), CompressionAlgorithm::Gzip));
        })
        .bench_function("decompress gzip", |b| {
            b.iter(|| {
                decompress(
                    black_box(compressed_gzip.as_slice()),
                    CompressionAlgorithm::Gzip,
                    max_size,
                )
            });
        })
        .bench_function("compress deflate", |b| {
            b.iter(|| compress(black_box(html_slice), CompressionAlgorithm::Deflate));
        })
        .bench_function("decompress deflate", |b| {
            b.iter(|| {
                decompress(
                    black_box(compressed_deflate.as_slice()),
                    CompressionAlgorithm::Deflate,
                    max_size,
                )
            });
        });
}

criterion_group!(compression, regex_capture_matches);
criterion_main!(compression);
