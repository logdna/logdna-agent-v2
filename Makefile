REPO := logdna-agent-v2

# The image repo and tag can be modified e.g.
# `make build RUST_IMAGE=docker.io/rust:latest
RUST_IMAGE_REPO ?= docker.io/logdna/build-images
RUST_IMAGE_TAG ?= rust-buster-stable
RUST_IMAGE ?= $(RUST_IMAGE_REPO):$(RUST_IMAGE_TAG)
RUST_IMAGE := $(RUST_IMAGE)

HADOLINT_IMAGE_REPO ?= hadolint/hadolint
HADOLINT_IMAGE_TAG ?= v1.18.0-debian
HADOLINT_IMAGE ?= $(HADOLINT_IMAGE_REPO):$(HADOLINT_IMAGE_TAG)
HADOLINT_IMAGE := $(HADOLINT_IMAGE)

SHELLCHECK_IMAGE_REPO ?= koalaman/shellcheck-alpine
SHELLCHECK_IMAGE_TAG ?= stable
SHELLCHECK_IMAGE ?= $(SHELLCHECK_IMAGE_REPO):$(SHELLCHECK_IMAGE_TAG)
SHELLCHECK_IMAGE := $(SHELLCHECK_IMAGE)

WORKDIR :=/build
DOCKER := DOCKER_BUILDKIT=1 docker
DOCKER_DISPATCH := ./docker/dispatch.sh "$(WORKDIR)" "$(shell pwd):/build:Z"
DOCKER_JOURNALD_DISPATCH := ./docker/journald_dispatch.sh "$(WORKDIR)" "$(shell pwd):/build:Z"
DOCKER_PRIVATE_IMAGE := us.gcr.io/logdna-k8s/logdna-agent-v2
DOCKER_PUBLIC_IMAGE := docker.io/logdna/logdna-agent
DOCKER_IBM_IMAGE := icr.io/ext/logdna-agent

export CARGO_CACHE ?= $(shell pwd)/.cargo_cache
RUST_COMMAND := $(DOCKER_DISPATCH) $(RUST_IMAGE)
HADOLINT_COMMAND := $(DOCKER_DISPATCH) $(HADOLINT_IMAGE)
SHELLCHECK_COMMAND := $(DOCKER_DISPATCH) $(SHELLCHECK_IMAGE)

INTEGRATION_TEST_THREADS ?= 1

VCS_REF := $(shell git rev-parse --short HEAD)
VCS_URL := https://github.com/logdna/$(REPO)
BUILD_DATE := $(shell date -u +'%Y%m%d')
BUILD_TIMESTAMP := $(shell date -u +'%Y-%m-%dT%H:%M:%SZ')
BUILD_VERSION := $(shell sed -nE "s/^version = \"(.+)\"\$$/\1/p" bin/Cargo.toml)
BUILD_TAG ?= $(VCS_REF)

MAJOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f1)
MINOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f2)
PATCH_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f3 | cut -d- -f1)
BETA_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d- -f2 | cut -s -d. -f2)
ifeq ($(BETA_VERSION),)
	BETA_VERSION := 0
endif

ifeq ($(ALL), 1)
	CLEAN_TAG := *
else
	CLEAN_TAG := $(BUILD_TAG)
endif

PULL ?= 1
ifeq ($(PULL), 1)
	PULL_OPTS := --pull
else
	PULL_OPTS :=
endif

CHANGE_BIN_VERSION = awk '{sub(/^version = ".+"$$/, "version = \"$(1)\"")}1' bin/Cargo.toml >> bin/Cargo.toml.tmp && mv bin/Cargo.toml.tmp bin/Cargo.toml

CHANGE_K8S_VERSION = sed 's/\(.*\)app\.kubernetes\.io\/version\(.\).*$$/\1app.kubernetes.io\/version\2 $(1)/g' $(2) >> $(2).tmp && mv $(2).tmp $(2)

CHANGE_K8S_IMAGE = sed 's/\(logdna\/logdna-agent.\).*$$/\1$(1)/g' $(2) >> $(2).tmp && mv $(2).tmp $(2)

REMOTE_BRANCH := $(shell git branch -vv | awk '/^\*/{split(substr($$4, 2, length($$4)-2), arr, "/"); print arr[2]}')

AWS_SHARED_CREDENTIALS_FILE=$(HOME)/.aws/credentials

LOGDNA_HOST?=localhost:1337

RUST_LOG?=info

_TAC= awk '{line[NR]=$$0} END {for (i=NR; i>=1; i--) print line[i]}'
TEST_RULES=
# Dynamically generate test targets for each workspace
define TEST_RULE
TEST_RULES=$(TEST_RULES)test-$(1): <> Run unit tests for $(1) crate\\n
test-$(1):
	$(RUST_COMMAND) "--env RUST_BACKTRACE=1" "cargo test -p $(1)"
endef

CRATES=$(shell sed -e '/members/,/]/!d' Cargo.toml | tail -n +2 | $(_TAC) | tail -n +2 | $(_TAC) | sed 's/,//' | xargs -n1 -I{} sh -c 'grep -E "^name *=" {}/Cargo.toml | tail -n1' | sed 's/name *= *"\([A-Za-z0-9_\-]*\)"/\1/' | awk '!/journald/{print $0}')
$(foreach _crate, $(CRATES), $(eval $(call TEST_RULE,$(strip $(_crate)))))

.PHONY:build
build: ## Build the agent
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo build"

.PHONY:build-release
build-release: ## Build a release version of the agent
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo build --release && strip ./target/release/logdna-agent"

.PHONY:check
check: ## Run unit tests
	$(RUST_COMMAND) "" "cargo check --all-targets"

.PHONY:test
test: test-journald ## Run unit tests
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "cargo test --no-run && cargo test"

.PHONY:integration-test
integration-test: ## Run integration tests using image with additional tools
	$(DOCKER_JOURNALD_DISPATCH) "--env LOGDNA_INGESTION_KEY=$(LOGDNA_INGESTION_KEY) --env LOGDNA_HOST=$(LOGDNA_HOST) --env RUST_BACKTRACE=full --env RUST_LOG=debug" "cargo test --manifest-path bin/Cargo.toml --features integration_tests -- --nocapture --test-threads=$(INTEGRATION_TEST_THREADS)"

.PHONY:test-journald
test-journald: ## Run journald unit tests
	$(DOCKER_JOURNALD_DISPATCH) "--env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "cargo test --manifest-path bin/Cargo.toml -p journald --features journald_tests -- --nocapture"

.PHONY:clean
clean: ## Clean all artifacts from the build process
	$(RUST_COMMAND) "" "rm -fr target/* \$$CARGO_HOME/registry/* \$$CARGO_HOME/git/*"

.PHONY:clean-docker
clean-docker: ## Cleans the intermediate and final agent images left over from the build-image target
	@# Clean any agent images, left over from the multi-stage build
	if [[ ! -z "$(shell docker images -q $(REPO):$(CLEAN_TAG))" ]]; then docker images -q $(REPO):$(CLEAN_TAG) | xargs docker rmi -f; fi

.PHONY:clean-all
clean-all: clean-docker clean ## Deep cleans the project and removed any docker images
	git clean -xdf

.PHONY:lint-format
lint-format: ## Checks for formatting errors
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo fmt -- --check"

.PHONY:lint-clippy
lint-clippy: ## Checks for code errors
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo clippy --all-targets -- -D warnings"

.PHONY:lint-audit
lint-audit: ## Audits packages for issues
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo audit"

.PHONY:lint-docker
lint-docker: ## Lint the Dockerfile for issues
	$(HADOLINT_COMMAND) "" "hadolint Dockerfile --ignore DL3006"

.PHONY:lint-shell
lint-shell: ## Lint the Dockerfile for issues
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/lib.sh"
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/dispatch.sh"
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/journald_dispatch.sh"

.PHONY:lint
lint: lint-docker lint-shell lint-format lint-clippy lint-audit ## Runs all the linters

.PHONY:release-major
release-major: ## Create a new major beta release and push to github
	$(eval TARGET_BRANCH := $(shell expr $(MAJOR_VERSION) + 1).0)
	$(eval NEW_VERSION := $(TARGET_BRANCH).0-beta.1)
	@if [ ! "$(REMOTE_BRANCH)" = "master" ]; then echo "Can't create the major beta release \"$(NEW_VERSION)\" on the remote branch \"$(REMOTE_BRANCH)\". Please checkout \"master\""; exit 1; fi
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags
	git checkout $(TARGET_BRANCH) || git checkout -b $(TARGET_BRANCH)

.PHONY:release-minor
release-minor: ## Create a new minor beta release and push to github
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(shell expr $(MINOR_VERSION) + 1))
	$(eval NEW_VERSION := $(TARGET_BRANCH).0-beta.1)
	@if [ ! "$(REMOTE_BRANCH)" = "master" ]; then echo "Can't create the minor beta release \"$(NEW_VERSION)\" on the remote branch \"$(REMOTE_BRANCH)\". Please checkout \"master\""; exit 1; fi
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags
	git checkout $(TARGET_BRANCH) || git checkout -b $(TARGET_BRANCH)

.PHONY:release-patch
release-patch: ## Create a new patch beta release and push to github
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(MINOR_VERSION))
	$(eval NEW_VERSION := $(TARGET_BRANCH).$(shell expr $(PATCH_VERSION) + 1))
	@if [ ! "$(REMOTE_BRANCH)" = "$(TARGET_BRANCH)" ]; then echo "Can't create the patch release \"$(NEW_VERSION)\" on the remote branch \"$(REMOTE_BRANCH)\". Please checkout \"$(TARGET_BRANCH)\""; exit 1; fi
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags
	git checkout $(TARGET_BRANCH) || git checkout -b $(TARGET_BRANCH)

.PHONY:release-beta
release-beta: ## Bump the beta version and push to github
	@if [ "$(BETA_VERSION)" = "0" ]; then echo "Can't create a new beta on top of an existing version, use release-[major|minor|patch] targets instead"; exit 1; fi
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(MINOR_VERSION))
	$(eval NEW_VERSION := $(TARGET_BRANCH).$(PATCH_VERSION)-beta.$(shell expr $(BETA_VERSION) + 1))
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags
	git checkout $(TARGET_BRANCH) || git checkout -b $(TARGET_BRANCH)

.PHONY:release
release: ## Create a new release from the current beta and push to github
	@if [ "$(BETA_VERSION)" = "0" ]; then echo "Can't release from a non-beta version"; exit 1; fi
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(MINOR_VERSION))
	$(eval NEW_VERSION := $(TARGET_BRANCH).0)
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "cargo generate-lockfile"
	git add Cargo.lock bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git tag -s -a $(NEW_VERSION) -m ""
	git push --follow-tags
	git checkout $(TARGET_BRANCH) || git checkout -b $(TARGET_BRANCH)

.PHONY:build-image
build-image: ## Build a docker image as specified in the Dockerfile
	$(DOCKER) build . -t $(REPO):$(BUILD_TAG) \
		--progress=plain \
		--secret id=aws,src=$(AWS_SHARED_CREDENTIALS_FILE) \
		--rm \
		--build-arg BUILD_IMAGE=$(RUST_IMAGE) \
		--build-arg BUILD_TIMESTAMP=$(BUILD_TIMESTAMP) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL) \
		--build-arg SCCACHE_BUCKET=$(SCCACHE_BUCKET) \
		--build-arg SCCACHE_REGION=$(SCCACHE_REGION)

.PHONY:publish-image
publish-image: ## Publish SemVer compliant releases to our registroies
	$(eval TARGET_VERSIONS := $(BUILD_VERSION) $(shell if [ "$(BETA_VERSION)" = "0" ]; then echo "$(BUILD_VERSION)-$(BUILD_DATE).$(shell docker images -q $(REPO):$(BUILD_TAG)) $(MAJOR_VERSION) $(MAJOR_VERSION).$(MINOR_VERSION)"; fi))
	@for image in $(DOCKER_PRIVATE_IMAGE) $(DOCKER_PUBLIC_IMAGE) $(DOCKER_IBM_IMAGE); do \
		set -e; \
		for version in $(TARGET_VERSIONS); do \
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
	@$(SHELL) -c "echo '$(TEST_RULES)'" |  awk 'BEGIN {FS = ":.*?<> "}; /^.+: .*?<> / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'
