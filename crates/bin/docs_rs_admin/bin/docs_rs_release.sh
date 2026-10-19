#!/usr/bin/env bash

set -euo pipefail

# Run after deploying the web service and waiting for healthy tasks.
# Included in the CLI image; override its admin-binary entrypoint to run this
# script as a one-off release task.
#
# Should only be run once per release, more than once doesn't hurt,
# but don't run in parallel.

docs_rs_admin database migrate

# purge static content that can only change on release.
# See `crates/bin/docs_rs_web` `cache::SURROGATE_KEY_DOCSRS_STATIC` and its
# usages.
if [ -n "${DOCSRS_FASTLY_API_TOKEN:-}" ] && [ -n "${DOCSRS_FASTLY_SERVICE_SID_WEB:-}" ]; then
    docs_rs_admin cdn purge docs-rs-static
else
    echo "Fastly is not configured; skipping static-content purge."
fi
