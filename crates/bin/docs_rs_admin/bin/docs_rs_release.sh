#!/usr/bin/env bash

set -eo pipefail

# all things that should happen after we deploy a new version.
# at some point, should be integrated into the docker containers, 
# AWS deploy, etc.  
#
# Should only be run once per release, more than once doesn't hurt, 
# but don't run in parallel.

# run database migrations.
DOCSRS_MIN_POOL_IDLE=1 DOCSRS_MAX_POOL_SIZE=10 docs_rs_admin database migrate

# purge static content that can only change on release. 
# See `crates/bin/docs_rs_web` `cache::SURROGATE_KEY_DOCSRS_STATIC` and its 
# usages.
docs_rs_admin cdn purge docs-rs-static
