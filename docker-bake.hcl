variable "GIT_SHA" {
  default = "dev"
}

variable "ECR_REGISTRY" {
  default = ""
}

# Use local tags for development and registry tags for publishing.
function "image_tags" {
  params = [repository]
  result = ECR_REGISTRY == "" ? ["${repository}:ci"] : [
    "${ECR_REGISTRY}/${repository}:${GIT_SHA}",
    "${ECR_REGISTRY}/${repository}:latest",
  ]
}

group "default" {
  targets = ["web-server", "build-server", "registry-watcher", "cli"]
}

target "common" {
  context    = "."
  dockerfile = "dockerfiles/Dockerfile"
  platforms  = ["linux/amd64"]
  output     = ["type=docker"]
  args = {
    GIT_SHA    = GIT_SHA
    PROFILE    = "release"
    PROFILE_DIR = "release"
  }
}

# Only targets with provisioned ECR repositories inherit this exporter.
target "publishable" {
  inherits = ["common"]
  output   = [ECR_REGISTRY == "" ? "type=docker" : "type=registry"]
}

target "web-server" {
  inherits = ["publishable"]
  target   = "web-server"
  tags     = image_tags("docs-rs-web")
}

target "build-server" {
  inherits = ["publishable"]
  target   = "build-server"
  tags     = image_tags("docs-rs-build-server")
}

target "registry-watcher" {
  inherits = ["publishable"]
  target   = "registry-watcher"
  tags     = image_tags("docs-rs-registry-watcher")
}

target "cli" {
  inherits = ["publishable"]
  target   = "cli"
  tags     = image_tags("docs-rs-admin")
}
