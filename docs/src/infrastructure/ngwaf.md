# Fastly NgWAF

We use the
[Fastly Web Application Firewall (NgWAF)](https://www.fastly.com/documentation/guides/next-gen-waf/).
It's integrated with our Fastly Compute WASM module, so all blocking happens in
the CDN and no malicious requests reach our origin servers.

When something is blocked, the user will see one of the following:

- status `406 NOT ACCEPTABLE` for normal security rules, or
- status `429 Too Many Requests` for rate limiting.

_These status codes are only used by the NgWAF, so if a user sees one, the
NgWAF is the component blocking the request._

## Changes and Deployment

The integration between the Fastly CDN and NgWAF is implemented in
[our Compute WASM module](https://github.com/rust-lang/simpleinfra/tree/master/terraform/docs-rs/fastly-compute-docs-rs/src/ngwaf.rs).

In the legacy architecture, the rules are defined manually in the
[Signal Sciences dashboard](https://dashboard.signalsciences.net/). With the
planned new infrastructure, we'll start managing these in Terraform as well.

_New or updated rules are typically distributed and active across Fastly's CDN
within one minute, though it can sometimes take two to three minutes._
