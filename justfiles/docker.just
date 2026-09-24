# Build all images or the selected Docker Bake targets for local use.
[group('docker')]
docker-build *targets:
    docker buildx bake --file docker-bake.hcl {{ targets }}

# Run --help in every image from the default Docker Bake group.
[group('docker')]
[group('testing')]
docker-smoke-test:
    #!/usr/bin/env bash
    set -euo pipefail

    docker buildx bake --file docker-bake.hcl --print |
      jq -r '
        . as $bake
        | $bake.group.default.targets[] as $target
        | $bake.target[$target].tags[0]
          // error("Bake target \($target) has no image tag")
      ' |
      while IFS= read -r image; do
        echo "testing $image..."
        docker run --rm "$image" --help
      done

# Build and smoke-test every image in the default Docker Bake group.
[group('docker')]
[group('testing')]
docker-test: docker-build docker-smoke-test
