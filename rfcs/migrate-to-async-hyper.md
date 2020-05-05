# Migrating docs.rs to asynchronous code and hyper

1. Update `tokio`, `futures` and `rusoto_{s3, core, credential}` to their latest versions (I think Joshua started on this, I'd be happy to help with finishing it)
2. Make `main` an `async fn` with `#[tokio::main]` to start the tokio runtime, add the `hyper` dependency and migrate the side services to async (By side services I mean the ones in the daemon)

Making the side services async sounds like a larger thing than it actually is, e.g.

```rust
thread::spawn(move || loop {
    thread::sleep(Duration::from_secs(60 * 60 * 6));
    if let Err(e) = github_updater() {
        error!("Failed to update github fields: {}", e);
    }
})
```

Would become

```rust
task::spawn(async move {
    let mut interval = time::interval(Duration::from_secs(60 * 60 * 6));
    loop {
        interval.tick().await;
        if let Err(e) = github_updater().await {
            error!("Failed to update github fields: {}", e);
        }
    }
})
```

This actually will do a lot for the server's performance, as the many threads that were doing nothing but sleeping can now be used for useful work. Additionally, for the `release activity updater` this is a good change, as instead un-sleeping every minute and checking if it's `23:55`, we can use tokio's `interval_at` to make it automatically wake once at `23:55` and execute.
Additionally, all the services are quite small and consist of basically http requests, so migrating can be literally as simple as adding awaits in some cases.

3. Now, at this point the next step would be to migrate from `iron` to `hyper`, and there's a few ways to go about it
    1. Move all routing to `hyper`, separate the handlers out from being within `iron` code and forwarding requests with `spawn_blocking`
        1. Requires some glue code, but adding that (in an inert state where it isn't used) can be done in separate prs
        2. Will be slightly slower until full migration is complete, as we'll have to translations from hyper requests to iron ones and from `IronResult` into the hyper equivalent
        3. Looking at the code, this will actually be made much easier by the fact that we already have a custom wrapper around managing routes, and we can use that to our advantage
    2. Use nginx to have two 'separate' servers on different ports that cross-forward things they can't handle
        1. Messy and kind of impractical, not to mention difficult to develop locally
        2. Probably the worst option
    3. Do it all at once
        1. Has the greatest code churn, but rips off the band-aid, so to speak
        2. The easiest and quickest option as far as developers are concerned, takes much more review time
    4. Have iron forward any requests it doesn't know how to handle to hyper and slowly move over handlers to hyper
        1. Very similar to option 1 except flipped, as instead of going hyper -> iron we go iron -> hyper
        2. Like option 1 it requires some initial setup/glue code that can be separated out into PRs
4. Here's where the database migration would happen, and again there are options
    1. Switch to `tokio-postgres`
        1. Virtually no change in actual code past changing imports and adding `.await`s
        2. Still requires *eventually* migrating to diesel
    2. Incrementally migrate to `diesel`/`tokio-diesel`
        1. The initial switch can be relatively small and can just involve switching migrations to diesel's format, making the diesel schemas and changing all queries to use `diesel::sql_query` (Allows using raw sql for queries, this lets us have a smaller change in code)
        2. (Not blocking for anything else, can be much smaller and more numerous PRs) Move existing queries to use actual diesel query building
        3. In the end I think this would be the better route, as it'd "force" us to actually migrate the database to the better option, as well as removing quite a few dependencies, namely `schemamam`, `r2d2` and `postgres`. Moving from those (postgres in particular) would also allow us to move other dependencies to better/newer options, such as changing the deprecated `time` crate to `chrono`. This would also force us to think a little more critically about how the database is structured and help with starting thinking about how to improve it (as there's some significant tech debt there, but that's a separate thing entirely)
