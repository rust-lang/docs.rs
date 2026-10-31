use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use docs_rs_storage::{FolderEntry, testing::open_archive_index};
use futures_util::TryStreamExt as _;
use std::{fs::File, hint::black_box, path::Path, sync::Arc};
use tokio::{runtime, sync::Mutex};

fn folder_contents(c: &mut Criterion) {
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let fixture_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shrust-0.0.4.index.zst");

    // Decompress once before timing, keeping the temporary index alive for all readers.
    let mut index_file = tempfile::NamedTempFile::new().unwrap();
    zstd::stream::copy_decode(File::open(fixture_path).unwrap(), &mut index_file)
        .expect("can't decompress fixture archive index");
    let index_path = index_file.into_temp_path();

    let mut index = runtime
        .block_on(open_archive_index(&index_path))
        .expect("can't open fixture archive index");

    let mut group = c.benchmark_group("folder_contents");
    for (name, folder) in [
        ("root", None),
        ("large_subtree", Some("gh-pages")),
        ("version", Some("gh-pages/v0.0.4")),
        ("many_files", Some("gh-pages/v0.0.4/libc")),
        ("small_folder", Some("examples")),
        ("missing", Some("nonexistent")),
    ] {
        // Warm the connection and verify the fixture before measuring repeated queries.
        let entries: Vec<FolderEntry> = runtime
            .block_on(index.folder_contents(folder.map(Path::new)).try_collect())
            .expect("can't list fixture folder");
        assert_eq!(entries.is_empty(), folder == Some("nonexistent"));

        group.bench_function(name, |b| {
            b.iter(|| {
                let entries: Vec<FolderEntry> = runtime
                    .block_on(
                        index
                            .folder_contents(black_box(folder.map(Path::new)))
                            .try_collect(),
                    )
                    .unwrap();
                black_box(entries)
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("folder_contents_concurrent");
    for readers in [1, 4, 16] {
        let indexes: Vec<_> = (0..readers)
            .map(|_| {
                Arc::new(Mutex::new(
                    runtime.block_on(open_archive_index(&index_path)).unwrap(),
                ))
            })
            .collect();

        for (name, folder) in [("root", None), ("many_files", Some("gh-pages/v0.0.4/libc"))] {
            group.bench_function(BenchmarkId::new(name, readers), |b| {
                b.iter(|| {
                    runtime.block_on(async {
                        let tasks = indexes.iter().map(|index| {
                            let index = Arc::clone(index);
                            runtime.spawn(async move {
                                let mut index = index.lock().await;
                                let entries: Vec<FolderEntry> = index
                                    .folder_contents(black_box(folder.map(Path::new)))
                                    .try_collect()
                                    .await
                                    .unwrap();
                                black_box(entries)
                            })
                        });
                        black_box(futures_util::future::try_join_all(tasks).await.unwrap())
                    })
                });
            });
        }
    }
    group.finish();
}

criterion_group!(folder_contents_benches, folder_contents);
criterion_main!(folder_contents_benches);
