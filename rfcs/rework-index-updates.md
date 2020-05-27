# Summary

This RFC proposes moving away from the current model of fetching new releases
to build, moving from running [crates-index-diff] in a thread to using webhooks
and the [crates-index] crate.

# Motivation

While the current approach has worked well for us so far, it has some problems:

* Running the update in a cronjob every 2 minutes is wasteful, as there is
  often a greater delay between two publishes.
* Running the update in a crobjob every 2 minutes adds delay to getting the
  documentation built if the queue is empty, as the release might potentially
  have to wait those extra two minutes.
* The approach doesn't scale, if we want to move to a setup where there is more
  than a single frontend server we'd have to elect which server runs the fetch.
* The way crates-index-diff stores its state (a branch in the local repo) is
  fragile, as it might become out of sync causing the loss of a publish.
* The way crates-index-diff stores its state makes it hard to move the server
  installation, as the index repository needs to be moved as well.

# Proposal

We configure the `crates.io-index` repository to send a webhook to a new
endpoint, `/_/index-webhook`, which starts a index sync in the background. The
payload of the webhook is ignored, but the webhook signature is validated if a
secret key is provided to the application through an environment variable.

When an index synchronization starts, the [crates-index] crate is used to load
in memory a list of all crates, their versions and whether each version is
yanked. Then, the full list of releases and queued crates is fetched from the
database, and it's compared with the contents of the index. Finally, idempotent
queries are sent to the database to update its state (queueing crates and
changing the yanked status) where needed.

# Rationale of the proposal

This proposal removes the cronjob and implements realtime updates of the index,
which does not have to happen on a specific machine if we ever move to multiple
frontend servers.

This proposal also works if multiple index synchronizations start at the
same time (for example, if two requests are received at the same time) without
having to implement a job queue: since all the updates to the database are
idempotent multiple syncs at the same time would not affect each other
(provided we structure the SQL queries the right way). A single mutex on each
host to lock `git fetch`es on the index might be needed though.

# Alternatives

We could implement only the webhook or the index synchronization, keeping the
old code for the part we don't replace. While it would improve the status quo,
it wouldn't address all the problems noted in the motivation.

We could also do nothing: while the current system is not perfect it works
without much trouble.

[crates-index-diff]: https://crates.io/crates/crates-index-diff
[crates-index]: https://crates.io/crates/crates-index
