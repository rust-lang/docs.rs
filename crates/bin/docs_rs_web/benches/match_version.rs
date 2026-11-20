use criterion::{Criterion, criterion_group, criterion_main};
use docs_rs_types::{ReqVersion, Version, testing::FOO};
use docs_rs_web::{Config, bench::match_version};
use futures_util::{StreamExt as _, stream};
use std::time::{Duration, Instant};
use tokio::runtime;

type TestEnvironment = docs_rs_context::testing::TestEnvironment<Config>;

const RELEASES: u64 = 2000;
const SEEDING_THREADS: usize = 7;

fn match_version_bench(c: &mut Criterion) {
    // silence logs, the test environment keeps an already set subscriber
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default()).unwrap();

    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let latest = Version::new(1, RELEASES - 1, 0);
    let middle = Version::new(1, RELEASES / 2, 0);

    eprintln!("seeding {RELEASES} releases of {FOO}...");
    let seed_start = Instant::now();
    let env = runtime.block_on(async {
        let env = TestEnvironment::new().await.unwrap();
        let create = |minor| {
            let env = &env;
            async move {
                env.fake_release()
                    .await
                    .name(FOO)
                    .version(Version::new(1, minor, 0))
                    .create()
                    .await
            }
        };

        stream::iter(0..RELEASES - 1)
            .for_each_concurrent(SEEDING_THREADS, |minor| async move {
                create(minor).await.unwrap();
            })
            .await;
        // last and alone, so its `latest_version_id` update sees all releases
        create(RELEASES - 1).await.unwrap();

        env
    });
    eprintln!("seeded in {:?}", seed_start.elapsed());

    let cases = vec![
        // fast path
        ("exact", middle.to_string(), Some(middle)),
        ("latest", "latest".into(), Some(latest.clone())),
        ("star", "*".into(), Some(latest.clone())),
        // all releases
        ("semver", ">=0".into(), Some(latest.clone())),
        // not released, falls back to `^1.0.1`
        ("unknown_exact", "1.0.1".into(), Some(latest)),
    ];
    let pool = env.pool().unwrap().clone();

    let mut group = c.benchmark_group("match_version");
    // iterations are database round trips, little to warm up
    group.warm_up_time(Duration::from_secs(1));
    // the full-list cases need ~9s for 100 samples with 2000 releases
    group.measurement_time(Duration::from_secs(10));
    for (name, req, expected) in cases {
        let req: ReqVersion = req.parse().unwrap();

        // setup asserts, see if `match_version` returns what we expect
        runtime.block_on(async {
            let mut conn = pool.get_async().await.unwrap();
            let version = match_version(&mut conn, &FOO, &req).await.unwrap();
            if let Some(expected) = expected {
                assert_eq!(version, expected, "{name}");
            }
        });

        let (pool, krate, req) = (&pool, &FOO, &req);
        group.bench_function(name, |b| {
            b.to_async(&runtime).iter_custom(|iters| async move {
                // using `iter_custom` so connection acquire isn't measured.
                let mut conn = pool.get_async().await.unwrap();

                let start = Instant::now();
                for _ in 0..iters {
                    match_version(&mut conn, krate, req).await.unwrap();
                }
                start.elapsed()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, match_version_bench);
criterion_main!(benches);
