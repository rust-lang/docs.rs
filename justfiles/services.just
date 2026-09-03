# Start the PostgreSQL and S3 resources used by local development.
[group('compose')]
compose-up-resources: _touch-docker-env
    # this just start up the "default" resources from `docker-compose.yml`
    docker compose up --detach --wait --remove-orphans

# Apply migrations, then start the requested Compose profiles.
[group('compose')]
compose-up *profiles: _touch-docker-env _ensure_db_migrated
    docker compose {{ prepend("--profile ", profiles) }} up --build -d --wait --remove-orphans

# Start the web-server Compose profile.
[group('compose')]
compose-up-web: (compose-up "web")

# Start the Compose profile containing two build servers.
[group('compose')]
compose-up-builder: (compose-up "builder")

# Start the registry-watcher Compose profile.
[group('compose')]
[group('watcher')]
compose-up-watcher: (compose-up "watcher")

# Start the OpenTelemetry metrics Compose profile.
[group('compose')]
compose-up-metrics: (compose-up "metrics")

# Start every application Compose profile.
[group('compose')]
compose-up-full: (compose-up "full")

# Stop Compose services while retaining local images and volumes.
[group('compose')]
compose-down:
    docker compose --profile full --profile manual down --remove-orphans

# Remove Compose services, images, volumes, and generated local artifacts.
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

# Follow logs from all Compose services or the specified services.
[group('compose')]
compose-logs *services:
    docker compose --profile full logs -f {{ services }}

# Run a command in a one-off Compose service container.
[group('cli')]
[group('compose')]
docker-run service_name *args: compose-up-resources
    # `docker compose run` ignores service dependencies, so the recipe starts
    # the database and object storage explicitly before reaching this point.
    docker compose run --build --rm {{ service_name }} {{ args }}
