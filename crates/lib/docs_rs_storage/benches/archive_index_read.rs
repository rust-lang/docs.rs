use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use docs_rs_storage::{
    FolderEntry,
    testing::{Index, new_threadpool},
};
use futures_util::{Stream, TryStreamExt as _, future::try_join_all};
use std::{fs::File, future::Future, hint::black_box, path::Path, sync::Arc};
use tokio::{runtime, sync::Mutex};

const SQLITE_THREADS: usize = 32;
const FILE_COUNT: u64 = 80_827;
const HIT: &str = "Cargo.toml";
const DEEP_HIT: &str = "gh-pages/v0.0.4/libc/index.html";
const MISS: &str = "nonexistent/file.html";

async fn folder_entries(
    entries: impl Stream<Item = anyhow::Result<FolderEntry>>,
) -> Vec<FolderEntry> {
    entries
        .try_collect()
        .await
        .expect("can't list fixture folder")
}

async fn run_concurrent<T: Send + 'static>(
    tasks: impl IntoIterator<Item = impl Future<Output = T> + Send + 'static>,
) {
    black_box(
        try_join_all(tasks.into_iter().map(tokio::spawn))
            .await
            .expect("archive index benchmark task failed"),
    );
}

fn archive_index_read(c: &mut Criterion) {
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let pool = new_threadpool(SQLITE_THREADS).expect("create benchmark SQLite pool");

    // Share the existing large directory-listing fixture. Decompression and the
    // initial reads are setup costs, not part of any measured operation.
    let fixture_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shrust-0.0.4.index.zst");
    let mut index_file = tempfile::NamedTempFile::new().unwrap();
    zstd::stream::copy_decode(File::open(fixture_path).unwrap(), &mut index_file).unwrap();
    let index_path = Arc::new(index_file.into_temp_path());
    let mut index = runtime
        .block_on(Index::open(index_path.as_ref(), pool.clone()))
        .unwrap();

    runtime.block_on(async {
        assert_eq!(index.file_count().await.unwrap(), FILE_COUNT);
        assert!(index.find(HIT).await.unwrap().is_some());
        assert!(index.find(DEEP_HIT).await.unwrap().is_some());
        assert!(index.find(MISS).await.unwrap().is_none());
        assert_eq!(
            index.list().try_collect::<Vec<_>>().await.unwrap().len() as u64,
            FILE_COUNT
        );
    });

    // Isolate query costs on a connection that is already open, with warm file
    // system caches. Folder query costs are covered by the folder_contents group below.
    let mut group = c.benchmark_group("archive_index_read/reused_connection");
    group.throughput(Throughput::Elements(1));
    for (name, path) in [
        ("find_hit", HIT),
        ("find_deep_hit", DEEP_HIT),
        ("find_miss", MISS),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| black_box(runtime.block_on(index.find(black_box(path))).unwrap()));
        });
    }
    group.bench_function("file_count", |b| {
        b.iter(|| black_box(runtime.block_on(index.file_count()).unwrap()));
    });
    group.throughput(Throughput::Elements(FILE_COUNT));
    group.bench_function("list_all", |b| {
        b.iter(|| {
            black_box(
                runtime
                    .block_on(index.list().try_collect::<Vec<_>>())
                    .unwrap(),
            )
        });
    });
    group.finish();

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
        let entries =
            runtime.block_on(folder_entries(index.folder_contents(folder.map(Path::new))));
        assert_eq!(entries.is_empty(), folder == Some("nonexistent"));

        group.bench_function(name, |b| {
            b.iter(|| {
                let entries = runtime.block_on(folder_entries(
                    index.folder_contents(black_box(folder.map(Path::new))),
                ));
                black_box(entries)
            });
        });
    }
    group.finish();

    drop(index);

    // Production opens a connection per lookup. Include open, PRAGMAs, query,
    // result consumption, and dropping the connection in each timed request.
    // This is a local-index benchmark: no S3 downloads or cache repairs occur.
    let mut group = c.benchmark_group("archive_index_read/fresh_connection");
    group.throughput(Throughput::Elements(1));
    group.bench_function("open", |b| {
        b.to_async(&runtime).iter(|| async {
            let index = Index::open(black_box(index_path.as_ref()), pool.clone())
                .await
                .unwrap();
            drop(black_box(index));
        });
    });
    group.finish();

    let mut group = c.benchmark_group("archive_index_read/fresh_connection_concurrent");
    for readers in [1, 4, 16, 64, 128, 256] {
        // Each iteration waits for a complete batch of independent requests.
        // Throughput counts requests, while Criterion reports batch latency.
        group.throughput(Throughput::Elements(readers));
        for (name, search_path) in [("find_hit", HIT), ("find_miss", MISS)] {
            group.bench_function(BenchmarkId::new(name, readers), |b| {
                b.to_async(&runtime).iter(|| {
                    run_concurrent((0..readers).map(|_| {
                        let path = Arc::clone(&index_path);
                        let pool = Arc::clone(&pool);
                        async move {
                            let mut index = Index::open(path.as_ref(), pool).await.unwrap();
                            black_box(index.find(black_box(search_path)).await.unwrap());
                        }
                    }))
                });
            });
        }
        group.bench_function(BenchmarkId::new("folder_root", readers), |b| {
            b.to_async(&runtime).iter(|| {
                run_concurrent((0..readers).map(|_| {
                    let path = Arc::clone(&index_path);
                    let pool = Arc::clone(&pool);
                    async move {
                        let mut index = Index::open(path.as_ref(), pool).await.unwrap();
                        black_box(folder_entries(index.folder_contents(None::<&Path>)).await);
                    }
                }))
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("folder_contents_concurrent");
    for readers in [1, 4, 16] {
        let indexes: Vec<_> = (0..readers)
            .map(|_| {
                Arc::new(Mutex::new(
                    runtime
                        .block_on(Index::open(index_path.as_ref(), pool.clone()))
                        .unwrap(),
                ))
            })
            .collect();

        for (name, folder) in [("root", None), ("many_files", Some("gh-pages/v0.0.4/libc"))] {
            group.bench_function(BenchmarkId::new(name, readers), |b| {
                b.to_async(&runtime).iter(|| {
                    run_concurrent(indexes.iter().map(|index| {
                        let index = Arc::clone(index);
                        async move {
                            let mut index = index.lock().await;
                            black_box(
                                folder_entries(
                                    index.folder_contents(black_box(folder.map(Path::new))),
                                )
                                .await,
                            )
                        }
                    }))
                });
            });
        }
    }
    group.finish();
}

criterion_group!(archive_index_read_benches, archive_index_read);
criterion_main!(archive_index_read_benches);
