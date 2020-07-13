REPO := $(shell git remote get-url origin | sed 's/\.git//' | cut -f2 -d'/')

RUST_IMAGE ?= docker.io/rust
RUST_VERSION ?= 1.44.1

DOCKER ?= docker
DOCKER_RUN = $(DOCKER) run --rm

CARGO = $(DOCKER_RUN) -w /usr/src/myapp -v $(shell pwd):/usr/src/myapp:Z $(RUST_IMAGE):$(RUST_VERSION) cargo
RUSTUP = rustup
RELEASE ?= 0
ifeq ($(RELEASE), 0)
	CARGO_COMPILE_OPTS =
else
	CARGO_COMPILE_OPTS = --release
endif

.PHONY:build
build:		## Build the agent. Set RELEASE=1 to build a release image - defaults to 0
	$(CARGO) build $(CARGO_COMPILE_OPTS)

.PHONY:clean
clean:		## Clean any artifacts from the build target
	$(CARGO) clean

.PHONY:test
test:		## Run unit tests and linters
	$(CARGO) fmt -- --check
	$(CARGO) clippy --all-targets $(CARGO_COMPILE_OPTS) -- -D warnings
	$(CARGO) +nightly udeps --all-targets $(CARGO_COMPILE_OPTS)
	$(CARGO) test $(CARGO_COMPILE_OPTS)

.PHONY:test-deps
test-deps:	## Install dependencies needed for the test target
	$(RUSTUP) update
	$(RUSTUP) toolchain install nightly
	$(RUSTUP) component add clippy
	$(RUSTUP) component add rustfmt
	$(CARGO) +nightly install cargo-udeps --locked

.PHONY:help
help:		## Prints out a helpful description of each possible target
	@awk 'BEGIN {FS = ":.*?## "}; /^.+: .*?## / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

.PHONY:build-image
BUILD_DATE=$(shell date -u +'%Y%m%dT%H%M%SZ')
BUILD_VERSION=$(shell git describe HEAD --tags --always)
VCS_REF=$(shell git rev-parse --short HEAD)
VCS_URL=$(shell git remote get-url origin)
build-image: clean	## Build a docker image as specified in the Dockerfile
	$(DOCKER) build . -t $(REPO):$(BUILD_DATE)-$(BUILD_VERSION) \
		--pull --no-cache=true \
		--build-arg BUILD_DATE=$(BUILD_DATE) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL)
	$(DOCKER) tag $(REPO):$(BUILD_DATE).$(BUILD_VERSION) $(REPO):$(BUILD_VERSION)

.PHONY:publish
publish:
	$(DOCKER) tag $(REPO):$(BUILD_VERSION) us.gcr.io/logdna-k8s/$(REPO):$(BUILD_VERSION)
	# TODO: Use the commitizen add-on for semver tagging of docker images
	$(DOCKER) push us.gcr.io/logdna-k8s/$(REPO):$(BUILD_VERSION)
