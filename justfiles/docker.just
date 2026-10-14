# Build and smoke-test all images or the selected Docker Bake targets locally.
[group('docker')]
docker-build *targets:
    docker buildx bake --file docker-bake.hcl {{ targets }}

# Build and smoke-test every image in the default Docker Bake group.
[group('docker')]
[group('testing')]
docker-test: docker-build
