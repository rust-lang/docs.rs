use anyhow::Result;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use docs_rs_opentelemetry::testing::TestMetrics;
use docs_rs_storage::{StorageKind, testing::TestStorage};
use docs_rs_types::BuildId;
use futures_util::future::try_join_all;
use std::{
    path::Path,
    sync::atomic::{AtomicI32, Ordering},
};
use tokio::{fs, runtime};

const ARCHIVE_PATH: &str = "bench/archive.zip";
const FILE_IN_ARCHIVE: &str = "Cargo.toml";

async fn write_fixture_files(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("src")).await?;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"bench\"\n").await?;
    fs::write(root.join("src/lib.rs"), "pub fn f() -> usize { 42 }\n").await?;
    Ok(())
}

async fn create_storage_and_archive() -> Result<TestStorage> {
    let metrics = TestMetrics::new();
    let storage = TestStorage::from_kind(StorageKind::Memory, metrics.provider()).await?;

    let fixture_dir = tempfile::tempdir()?;
    write_fixture_files(fixture_dir.path()).await?;

    storage
        .store_all_in_archive(ARCHIVE_PATH, fixture_dir.path())
        .await?;

    Ok(storage)
}

pub fn archive_index_cache(c: &mut Criterion) {
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let storage = runtime
        .block_on(create_storage_and_archive())
        .expect("can't create test storage & archive");

    runtime.block_on(async {
        assert!(
            storage
                .exists_in_archive(ARCHIVE_PATH, Some(BuildId(1)), FILE_IN_ARCHIVE)
                .await
                .expect("initial exists-call failed")
        );
    });

    let mut group = c.benchmark_group("archive_index_cache");

    group.bench_function(
        BenchmarkId::new("hot_local_index_single", "exists_in_archive"),
        |b| {
            b.to_async(&runtime).iter(|| async {
                assert!(
                    storage
                        .exists_in_archive(ARCHIVE_PATH, Some(BuildId(1)), FILE_IN_ARCHIVE)
                        .await
                        .unwrap()
                );
            });
        },
    );

    let cold_counter = AtomicI32::new(10_000);
    group.bench_function(
        BenchmarkId::new("cold_index_single", "exists_in_archive"),
        |b| {
            b.to_async(&runtime).iter(|| async {
                let build_id = BuildId(cold_counter.fetch_add(1, Ordering::Relaxed));
                assert!(
                    storage
                        .exists_in_archive(ARCHIVE_PATH, Some(build_id), FILE_IN_ARCHIVE)
                        .await
                        .unwrap()
                );
            });
        },
    );

    let concurrent_counter = AtomicI32::new(20_000);
    group.bench_function(
        BenchmarkId::new("cold_index_concurrent_same_key_16", "exists_in_archive"),
        |b| {
            b.to_async(&runtime).iter(|| async {
                let build_id = BuildId(concurrent_counter.fetch_add(1, Ordering::Relaxed));
                let futures = (0..16).map(|_| {
                    storage.exists_in_archive(ARCHIVE_PATH, Some(build_id), FILE_IN_ARCHIVE)
                });

                let results = try_join_all(futures).await.unwrap();
                assert!(results.into_iter().all(std::convert::identity));
            });
        },
    );

    let recover_counter = AtomicI32::new(30_000);
    group.bench_function(
        BenchmarkId::new("purge_then_recover_single", "exists_in_archive"),
        |b| {
            b.to_async(&runtime).iter(|| async {
                let build_id = BuildId(recover_counter.fetch_add(1, Ordering::Relaxed));
                assert!(
                    storage
                        .exists_in_archive(ARCHIVE_PATH, Some(build_id), FILE_IN_ARCHIVE)
                        .await
                        .unwrap()
                );

                let local_index_path = storage
                    .config()
                    .archive_index_cache
                    .path
                    .join(format!("{ARCHIVE_PATH}.{}.index", build_id.0));
                let _ = fs::remove_file(&local_index_path).await;

                assert!(
                    storage
                        .exists_in_archive(ARCHIVE_PATH, Some(build_id), FILE_IN_ARCHIVE)
                        .await
                        .unwrap()
                );
            });
        },
    );

    group.finish();
}

criterion_group!(archive_index_cache_benches, archive_index_cache);
criterion_main!(archive_index_cache_benches);
