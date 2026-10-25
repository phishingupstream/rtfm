variable "CI_COMMIT_SHA" {
  default = "local"
}

variable "VCS_REF" {
  default = "development"
}

variable "IMAGE_REPOSITORY" {
  default = "rtfm-cli"
}

# Shared CI may supply publication settings through an additional Bake file.
target "_publish" {}

group "default" {
  targets = ["app"]
}

target "app" {
  inherits   = ["_publish"]
  context    = "."
  dockerfile = "Dockerfile"
  args       = { VCS_REF = VCS_REF }
  tags       = ["${IMAGE_REPOSITORY}:${CI_COMMIT_SHA}"]
}
