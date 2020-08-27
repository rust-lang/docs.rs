# Summary

This MCP proposes moving away from the current model of fetching new releases
to build, moving from using a timer to receiving webhooks.

# Motivation

While the current approach has worked well for us so far, it has some problems:

* Running the update in a timer every 2 minutes is wasteful, as there is
  often a greater delay between two publishes.
* Running the update in a timer every 2 minutes adds delay to getting the
  documentation built if the queue is empty, as the release might potentially
  have to wait those extra two minutes.
* The way crates-index-diff stores its state (a branch in the local repo) is
  fragile, as it might become out of sync causing the loss of a publish.
* The way crates-index-diff stores its state makes it hard to move the server
  installation, as the index repository needs to be moved as well.

# Proposal

We configure the `crates.io-index` repository to send a webhook to a new
endpoint, `/_/index-webhook`, which starts a index sync in the background. The
payload of the webhook is ignored, but the webhook signature is validated if a
secret key is provided to the application through an environment variable.

We also change [crate-index-diff] to store the hash of the last visited commit
in the database instead of a local branch in the index repository: this will
allow new instances to catch up immediately without the need of copying over
the git repository.

For this proposal to work we need to make the updates to the queue idempotent,
and add a lock on the index repository in each machine to prevent the same
machine from updating the same repository multiple times.

# Rationale of the proposal

This proposal removes the timer and implements realtime updates of the index,
which does not have to happen on a specific machine if we ever move to multiple
frontend servers.

# Alternatives

We could also switch from [crates-index-diff] to doing a full synchronization
every time a new crate is published. While it would decrease the chances of an
inconsistency between crates.io and docs.rs, it would impact performance every
time a new crate is published.

We could also do nothing: while the current system is not perfect it works
without much trouble.

[crates-index-diff]: https://crates.io/crates/crates-index-diff
