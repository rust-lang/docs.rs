
_ensure_db_and_s3_are_running: _touch-docker-env
  # dependencies in the docker-cli file are ignored 
  # here. Instead we explicitly start any dependent services first.
  docker compose up -d db s3 --wait

_touch-docker-env:
  touch .docker.env

