# run migrations, then launch one or more docker compose profiles in the background
[group('compose')]
compose-up *profiles: _touch-docker-env cli-db-migrate
  docker compose {{ prepend("--profile ", profiles) }} up --build -d --wait --remove-orphans

# Launch web server in the background
[group('compose')]
compose-up-web: 
  just compose-up web

# Launch two build servers in the background 
[group('compose')]
[group('builder')]
compose-up-builder: 
  just compose-up builder

# Launch registry watcher in the background 
[group('compose')]
[group('watcher')]
compose-up-watcher: 
  just compose-up watcher

# Launch metrics collector in the background 
[group('compose')]
compose-up-metrics: 
  just compose-up metrics

# Launch everything, all at once, in the background
[group('compose')]
compose-up-full:
  just compose-up full

# Shutdown docker services, keep containers & volumes alive.
[group('compose')]
compose-down:
  docker compose --profile full --profile manual down --remove-orphans

# Shutdown docker services, then clean up docker images, volumes & other local artifacts from this docker-compose project
[group('compose')]
compose-down-and-wipe:
  #!/usr/bin/env bash
  set -euo pipefail

  docker compose --profile full --profile manual down --volumes --remove-orphans --rmi local

  # When testing this in CI, I had permission issues when trying to remove this folder. 
  # Likely it's related to the docker container runnning as a different (root?) user, so 
  # these files in the `ignored/` folder belong to `root`. 

  # so we just try if we can use passwordless `sudo`: 
  if sudo -n true 2>/dev/null; then
    echo "deleting ignored/ folder with sudo"
    sudo -n rm -rf ignored/
  else
    echo "trying to delete ignored/ folder with current user."
    rm -rf ignored/ || { echo 'Failed to remove ignored/ - skipping' >&2; }
  fi

  mkdir -p ignored

# stream logs from all services running in docker-compose. Optionally specify services to tail logs from.
[group('compose')]
compose-logs *services:
  docker compose --profile full logs -f {{ services }}
