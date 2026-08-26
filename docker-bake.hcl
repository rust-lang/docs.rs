variable "GIT_SHA" {
  default = "dev"
}

group "default" {
  targets = ["web-server", "build-server", "registry-watcher", "cli"]
}

target "common" {
  context    = "."
  dockerfile = "dockerfiles/Dockerfile"
  platforms  = ["linux/amd64"]
  # Load images into the local Docker daemon so CI can smoke-test them.
  output     = ["type=docker"]
  args = {
    GIT_SHA    = GIT_SHA
    PROFILE    = "release"
    PROFILE_DIR = "release"
  }
}

target "web-server" {
  inherits = ["common"]
  target   = "web-server"
  tags     = ["docs-rs-web-server:ci"]
}

target "build-server" {
  inherits = ["common"]
  target   = "build-server"
  tags     = ["docs-rs-build-server:ci"]
}

target "registry-watcher" {
  inherits = ["common"]
  target   = "registry-watcher"
  tags     = ["docs-rs-registry-watcher:ci"]
}

target "cli" {
  inherits = ["common"]
  target   = "cli"
  tags     = ["docs-rs-cli:ci"]
}
