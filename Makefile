CARGO = cargo
DOCKER = docker
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
	git clean -xdf

.PHONY:test
test:		## Run unit tests and linters
	$(CARGO) clippy --all-targets $(CARGO_COMPILE_OPTS) -- -D warnings
	$(CARGO) fmt -- --check
	$(CARGO) test $(CARGO_COMPILE_OPTS)

.PHONY:help
help:		## Prints out a helpful description of each possible target
	@awk 'BEGIN {FS = ":.*?## "}; /^.+: .*?## / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' ${MAKEFILE_LIST}

.PHONY:docker
docker:		## Build a docker image as specified in the Dockerfile
	$(DOCKER) build .
