RUST_VERSION ?= 1.44.1
RUST_IMAGE = docker.io/rust
DOCKER_RUN = docker run --rm
# --user "$(shell id -u)":"$(shell id -g)"

CARGO = ${DOCKER_RUN} -w /usr/src/myapp -v $(shell pwd):/usr/src/myapp:Z ${RUST_IMAGE}:${RUST_VERSION} cargo
DOCKER = docker
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
	@awk 'BEGIN {FS = ":.*?## "}; /^.+: .*?## / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' ${MAKEFILE_LIST}

.PHONY:docker
docker:		## Build a docker image as specified in the Dockerfile
	$(DOCKER) build .
