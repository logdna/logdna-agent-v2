REPO := logdna-agent-v2

# The image repo and tag can be modified e.g.
# `make build IMAGE=docker.io/rust:1.42.0`
IMAGE_REPO ?= docker.io/rust
IMAGE_TAG ?= 1.42
IMAGE ?= $(IMAGE_REPO):$(IMAGE_TAG)
IMAGE := $(IMAGE)

DOCKER := docker
DOCKER_RUN := docker run --rm -w /build -v $(shell pwd):/build:Z $(IMAGE)
DOCKER_PRIVATE_IMAGE := us.gcr.io/logdna-k8s/logdna-agent-v2
DOCKER_PUBLIC_IMAGE := docker.io/logdna/logdna-agent
DOCKER_IBM_IMAGE := icr.io/ext/logdna-agent

VCS_REF := $(shell git rev-parse --short HEAD)
VCS_URL := https://github.com/logdna/$(REPO)
BUILD_TIMESTAMP := $(shell date -u +'%Y-%m-%dT%H:%M:%SZ')
BUILD_VERSION := $(shell sed -nE "s/^version = \"(.+)\"\$$/\1/p" bin/Cargo.toml)
BUILD_TAG := $(VCS_REF)

MAJOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f1)
MINOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f2)
PATCH_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f3 | cut -d- -f1)
BETA_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d- -f2 | cut -s -d. -f2)
ifeq ($(BETA_VERSION),)
	BETA_VERSION := 0
endif

ENTRYPOINT := ./docker/docker-entrypoint.sh $(shell id -u) $(shell id -g) "."

ifeq ($(ALL), 1)
	CLEAN_TAG := *
else
	CLEAN_TAG := $(BUILD_TAG)
endif

CLEAN_DOCKER_IMAGES = if [[ ! -z "docker images -q $(1)" ]]; then docker images -q $(1) | xargs docker rmi -f; fi

PULL ?= 1
ifeq ($(PULL), 1)
	PULL_OPTS := --pull
else
	PULL_OPTS :=
endif

CHANGE_VERSION = awk '{sub(/^version = ".+"$$/, "version = \"$(1)\"")}1' bin/Cargo.toml >> bin/Cargo.toml.tmp && mv bin/Cargo.toml.tmp bin/Cargo.toml

.PHONY:build
build: ## Build the agent
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo build"

.PHONY:build-release
build-release: ## Build a release version of the agent
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo build --release && strip ./target/release/logdna-agent"

.PHONY:test
test: ## Run unit tests
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo test"

.PHONY:clean
clean: ## Clean all artifacts from the build process
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo clean"

.PHONY:clean-docker
clean-docker: ## Cleans the intermediate and final agent images left over from the build-image target
	@# Clean any agent images, left over from the multi-stage build
	$(call CLEAN_DOCKER_IMAGES,$(REPO):$(CLEAN_TAG))

.PHONY:clean-all
clean-all: clean-docker ## Deep cleans the project and removed any docker images
	git clean -xdf

.PHONY:lint-format
lint-format: ## Checks for formatting errors
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo fmt -- --check"

.PHONY:lint-clippy
lint-clippy: ## Checks for code errors
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo clippy --all-targets -- -D warnings"

.PHONY:lint-audit
lint-audit: ## Audits packages for issues
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo audit"

.PHONY:lint
lint: ## Runs all the linters
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo fmt -- --check && cargo clippy --all-targets -- -D warnings && cargo audit"

.PHONY:release-major
release-major: ## Create a new major beta release and push to github
	$(eval NEW_VERSION := $(shell expr $(MAJOR_VERSION) + 1).0.0-beta.1)
	$(call CHANGE_VERSION,$(NEW_VERSION))
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags

.PHONY:release-minor
release-minor: ## Create a new minor beta release and push to github
	$(eval NEW_VERSION := $(MAJOR_VERSION).$(shell expr $(MINOR_VERSION) + 1).0-beta.1)
	$(call CHANGE_VERSION,$(NEW_VERSION))
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags

.PHONY:release-patch
release-patch: ## Create a new patch beta release and push to github
	$(eval NEW_VERSION := $(MAJOR_VERSION).$(MINOR_VERSION).$(shell expr $(PATCH_VERSION) + 1)-beta.1)
	$(call CHANGE_VERSION,$(NEW_VERSION))
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags

.PHONY:release-beta
release-beta: ## Bump the beta version and push to github
	@if [ "$(BETA_VERSION)" = "0" ]; then echo "Can't create a new beta on top of an existing version, use release-[major|minor|patch] targets instead"; exit 1; fi
	$(eval NEW_VERSION := $(MAJOR_VERSION).$(MINOR_VERSION).$(PATCH_VERSION)-beta.$(shell expr $(BETA_VERSION) + 1))
	$(call CHANGE_VERSION,$(NEW_VERSION))
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags

.PHONY:release
release: ## Create a new release from the current beta and push to github
	@if [ "$(BETA_VERSION)" = "0" ]; then echo "Can't release from a non-beta version"; exit 1; fi
	$(eval NEW_VERSION := $(MAJOR_VERSION).$(MINOR_VERSION).$(PATCH_VERSION))
	$(call CHANGE_VERSION,$(NEW_VERSION))
	$(DOCKER_RUN) $(ENTRYPOINT) "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags

.PHONY:build-image
build-image: ## Build a docker image as specified in the Dockerfile
	$(DOCKER) build . -t $(REPO):$(BUILD_TAG) \
		$(PULL_OPTS) --no-cache=true --rm \
		--build-arg BUILD_IMAGE=$(IMAGE) \
		--build-arg BUILD_TIMESTAMP=$(BUILD_TIMESTAMP) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL)
	$(DOCKER) tag $(REPO):$(BUILD_TAG) $(REPO):$(BUILD_VERSION)

.PHONY:publish-image
publish: ## Publish SemVer compliant releases to our registroies
	for image in $(DOCKER_PUBLIC_IMAGE) $(DOCKER_IBM_IMAGE) $(DOCKER_PRIVATE_IMAGE); do \
		for version in $(MAJOR_VERSION) $(MINOR_VERSION) $(PATCH_VERSION) $(BUILD_VERSION) latest; do \
			$(DOCKER) tag $(REPO):$(BUILD_TAG) $${image}:$${version}; \
			$(DOCKER) push $${image}:$${version}; \
		done; \
	done;

.PHONY:run
run: ## Run the debug version of the agent
	./target/debug/logdna-agent

.PHONY:run-release
run-release: ## Run the release version of the agent
	./target/release/logdna-agent

.PHONY:help
help: ## Prints out a helpful description of each possible target
	@awk 'BEGIN {FS = ":.*?## "}; /^.+: .*?## / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
