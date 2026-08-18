# Fastly CDN

Within the CDN, we run a
[Fastly Compute WASM module](https://www.fastly.com/documentation/guides/compute/developer-guides/rust/).
The code lives in our
[`simpleinfra` repository](https://github.com/rust-lang/simpleinfra/tree/master/terraform/docs-rs/fastly-compute-docs-rs).

This enables us to move performance-critical logic to the edge and write
integration tests for it.

The Compute module uses [Fastly NgWAF](ngwaf.md) to block malicious requests
at the CDN before they reach our origin servers.

[The Fastly service is configured via Terraform in the same
repository](https://github.com/rust-lang/simpleinfra/blob/master/terraform/docs-rs/fastly.tf).

What content is cached is defined solely by the `Cache-Control` headers that our
web server returns. There should not be any cache rules in the CDN module. For
now, we also don't want any business logic in the CDN, which makes the web
server easier to test and manage.

We also use the
[Fastly origin shield](https://www.fastly.com/documentation/guides/getting-started/hosts/shielding/)
to reduce the load on our web servers.

## Changes and Deployment

We typically make changes in
[the `simpleinfra` repository](https://github.com/rust-lang/simpleinfra/tree/master/terraform/docs-rs/),
after which they are reviewed and _manually_ applied by the infrastructure team.
