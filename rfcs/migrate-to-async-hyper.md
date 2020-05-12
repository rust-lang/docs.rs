# Migrating docs.rs to asynchronous code and hyper

## Approach

1. Update `tokio`, `futures` and `rusoto_{s3, core, credential}` to their latest versions

2. Make `main` an `async fn` with `#[tokio::main]` to start the tokio runtime, add the `hyper` dependency and migrate the side services to async (By side services I mean the ones in the daemon)

3. Add a catch-all handler that uses `hyper`, so that any requests not picked up by `iron` are handled by hyper
   1. Routes should be migrated to asynchronous code from least used to most used

4. Switch from `postgres` to `tokio-postgres` for database interaction

## Rationale

### Why async

A web server is inherently io bound, which is the type of task that asynchronous execution excels at. If all goes well, the server will run faster and with potentially less resource usage than it currently has due to using a single runtime to execute all operations.
Some of our current dependencies already use async (such as `rusoto`), and we could also potentially see improvements from them executing in an actually asynchronous environment.
Additionally, updating things allows us to both gain newer (and generally faster, safer and better supported) dependencies, as well as letting us give a critical eye to our current infrastructure to see what's good and what's bad.

### The approach

1. Updating all async-related dependencies gives a foothold to start the update process. Updating all of the dependencies at once is required because their versioning is intertwined

2. All functions down the 'pipeline' need to be slowly migrated to async, as you need async code to effectively call async code. At this point we can also migrate the side services to async, which will help with the overall efficiency of the server, as the background tasks that were just spinning on their own thread now are effectively scheduled by the tokio runtime.

3. Using a catch-all for hyper allows us to incrementally transfer from `iron` to `hyper` with minimal code disruption, as well as making the process of migration less complex and more easily reversible since we can do things one at a time.
   1. Migrating less used routes first allows us to potentially catch ill side-effects before they affect more critical code

4. Until this point all calls to postgres will be synchronous (Likely using `spawn_blocking`), but that's not the most efficient way to deal with an io type of task. Switching to `tokio-postgres` allows the minimum of code change while also gaining the benefits of multi-tasking.
   1. The actual change in code will be rather small, mostly just adding `.await`s and removing the `spawn_blocking`s that were used before

## Alternatives

Alternative migration strategies are as follows

1. Use nginx to have two 'separate' servers on different ports that cross-forward things they can't handle
   1. Messy, complicated and impractical
   2. Is a large drain and detriment to developers trying to work during the transition period

2. Migrate all at once
   1. Has the greatest code churn but gets things done with as soon as possible
   2. In the event that something goes wrong and the changes need to be reverted, much more development and review time will have been wasted

3. Have iron forward any requests it doesn't know how to handle to hyper and slowly move over handlers to hyper
   1. Very similar to the chosen option, except it would be much more complicated to actually get asynchronous execution started, causing us to not see any real changes until every single route was migrated over, potentially having the side effect of wasting all of the used time in the event it needs to be reverted
