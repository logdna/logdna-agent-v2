REPO := logdna-agent-v2

SHELLFLAGS := -ic

# The target architecture the agent is to be compiled for
export ARCH ?= x86_64
# The image repo and tag can be modified e.g.
# `make build RUST_IMAGE=docker.io/rust:latest
RUST_IMAGE_REPO ?= docker.io/logdna/build-images
RUST_IMAGE_BASE ?= buster
RUST_IMAGE_TAG ?= rust-$(RUST_IMAGE_BASE)-1-stable
RUST_IMAGE ?= $(RUST_IMAGE_REPO):$(RUST_IMAGE_TAG)-$(ARCH)

RUST_IMAGE_SUFFIX ?=
ifneq ($(RUST_IMAGE_SUFFIX),)
	RUST_IMAGE := $(RUST_IMAGE)-$(RUST_IMAGE_SUFFIX)
endif

BENCH_IMAGE_BASE ?= bullseye
BENCH_IMAGE_TAG ?= rust-$(BENCH_IMAGE_BASE)-1-stable
BENCH_IMAGE ?= $(RUST_IMAGE_REPO):$(BENCH_IMAGE_TAG)-$(ARCH)

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
DOCKER_DISPATCH := ARCH=$(ARCH) ./docker/dispatch.sh "$(WORKDIR)" "$(shell pwd):/build:Z"
DOCKER_JOURNALD_DISPATCH := BUILD_IMAGE=${RUST_IMAGE} ARCH=$(ARCH) ./docker/journald_dispatch.sh "$(WORKDIR)" "$(shell pwd):/build:Z"
DOCKER_KIND_DISPATCH := BUILD_IMAGE=${RUST_IMAGE} ARCH=$(ARCH) ./docker/kind_dispatch.sh "$(WORKDIR)" "$(shell pwd):/build:Z"
DOCKER_PRIVATE_IMAGE := us.gcr.io/logdna-k8s/logdna-agent-v2
DOCKER_PUBLIC_IMAGE ?= docker.io/logdna/logdna-agent
DOCKER_IBM_IMAGE := icr.io/ext/logdna-agent

export CARGO_CACHE ?= $(shell pwd)/.cargo_cache
RUST_COMMAND := $(DOCKER_DISPATCH) $(RUST_IMAGE)
UNCACHED_RUST_COMMAND := CACHE_TARGET="false" $(DOCKER_DISPATCH) $(RUST_IMAGE)
DEB_COMMAND := CACHE_TARGET="false" $(DOCKER_DISPATCH) alanfranz/fpm-within-docker:debian-bullseye
RPM_COMMAND := CACHE_TARGET="false" $(DOCKER_DISPATCH) alanfranz/fpm-within-docker:centos-8
BENCH_COMMAND = CACHE_TARGET="false" $(DOCKER_DISPATCH) $(BENCH_IMAGE)
HADOLINT_COMMAND := $(DOCKER_DISPATCH) $(HADOLINT_IMAGE)
SHELLCHECK_COMMAND := $(DOCKER_DISPATCH) $(SHELLCHECK_IMAGE)

INTEGRATION_TEST_THREADS ?= 1
K8S_TEST_CREATE_CLUSTER ?= true

VCS_REF := $(shell git rev-parse --short HEAD)
VCS_URL := https://github.com/logdna/$(REPO)
BUILD_DATE := $(shell date -u +'%Y%m%d')
BUILD_TIMESTAMP := $(shell date -u +'%Y-%m-%dT%H:%M:%SZ')
BUILD_VERSION := $(shell sed -nE "s/^version = \"(.+)\"\$$/\1/p" bin/Cargo.toml)
BUILD_TAG ?= $(VCS_REF)
IMAGE_TAG := $(BUILD_TAG)-$(ARCH)

MAJOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f1)
MINOR_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f2)
PATCH_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d. -f3 | cut -d- -f1)
BETA_VERSION := $(shell echo $(BUILD_VERSION) | cut -s -d- -f2 | cut -s -d. -f2)

TARGET_TAG ?= $(BUILD_VERSION)

ifeq ($(BETA_VERSION),)
	BETA_VERSION := 0
endif

ifeq ($(ALL), 1)
	CLEAN_TAG := *
else
	CLEAN_TAG := $(IMAGE_TAG)
endif

PULL ?= 1
ifeq ($(PULL), 1)
	PULL_OPTS := --pull
else
	PULL_OPTS :=
endif

STATIC ?= 0
ARCH_TRIPLE?=$(ARCH)-linux-gnu
TARGET?=$(ARCH)-unknown-linux-gnu
STATIC ?= 0
ifeq ($(STATIC), 1)
	ARCH_TRIPLE=$(ARCH)-linux-musl
	RUSTFLAGS:=-C link-self-contained=yes -Ctarget-feature=+crt-static -Clink-arg=-static -Clink-arg=-static-libstdc++ -Clink-arg=-static-libgcc -L /usr/local/$(ARCH)-linux-musl/lib/ -l static=stdc++ $(RUSTFLAGS)
	BINDGEN_EXTRA_CLANG_ARGS:=-I /usr/local/$(ARCH)-linux-musl/include
	TARGET=$(ARCH)-unknown-linux-musl
	BUILD_ENVS=ROCKSDB_LIB_DIR=/usr/local/rocksdb/$(ARCH)-linux-musl/lib ROCKSDB_INCLUDE_DIR=/usr/local/rocksdb/$(ARCH)-linux-musl/include ROCKSDB_STATIC=1 JEMALLOC_SYS_WITH_LG_PAGE=16
else
	RUSTFLAGS:=
endif

# Should we profile the benchmarks
PROFILE?=--profile

CHANGE_BIN_VERSION = awk '{sub(/^version = ".+"$$/, "version = \"$(1)\"")}1' bin/Cargo.toml >> bin/Cargo.toml.tmp && mv bin/Cargo.toml.tmp bin/Cargo.toml

CHANGE_K8S_VERSION = sed 's/\(.*\)app\.kubernetes\.io\/version\(.\).*$$/\1app.kubernetes.io\/version\2 $(1)/g' $(2) >> $(2).tmp && mv $(2).tmp $(2)

CHANGE_K8S_IMAGE = sed 's/\(logdna\/logdna-agent.\).*$$/\1$(1)/g' $(2) >> $(2).tmp && mv $(2).tmp $(2)

REMOTE_BRANCH := $(shell git branch -vv | awk '/^\*/{split(substr($$4, 2, length($$4)-2), arr, "/"); print arr[2]}')

AWS_SHARED_CREDENTIALS_FILE=$(HOME)/.aws/credentials

LOGDNA_HOST?=localhost:1337

RUST_LOG?=info

space := $(subst ,, )
comma := ,
FEATURES?=libjournald
FEATURES_ARG=$(if $(FEATURES),--features $(subst $(space),$(comma),$(FEATURES)))

join-with = $(subst $(space),$1,$(strip $2))

_TAC= awk '{line[NR]=$$0} END {for (i=NR; i>=1; i--) print line[i]}'
TEST_RULES=

# Dynamically generate test targets for each workspace
define TEST_RULE
TEST_RULES=$(TEST_RULES)test-$(1): <> Run unit tests for $(1) crate\\n
test-$(1):
	$(RUST_COMMAND) "--env RUST_BACKTRACE=1 --env RUST_LOG=$(RUST_LOG)" "cargo test -p $(1) $(TESTS) -- --nocapture"
endef

CRATES=$(shell sed -e '/members/,/]/!d' Cargo.toml | tail -n +2 | $(_TAC) | tail -n +2 | $(_TAC) | sed 's/,//' | xargs -n1 -I{} sh -c 'grep -E "^name *=" {}/Cargo.toml | tail -n1' | sed 's/name *= *"\([A-Za-z0-9_\-]*\)"/\1/' | awk '!/journald/{print $0}')
$(foreach _crate, $(CRATES), $(eval $(call TEST_RULE,$(strip $(_crate)))))

BUILD_ENV_DOCKER_ARGS=
ifneq ($(BUILD_ENVS),)
	BUILD_ENV_DOCKER_ARGS= --env $(call join-with, --env ,$(BUILD_ENVS))
endif

ifneq ($(TARGET),)
	TARGET_DOCKER_ARG= --target $(TARGET)
endif

.PHONY:build
build: ## Build the agent
	$(UNCACHED_RUST_COMMAND) "$(BUILD_ENV_DOCKER_ARGS) --env RUST_BACKTRACE=full" "RUSTFLAGS='$(RUSTFLAGS)' BINDGEN_EXTRA_CLANG_ARGS='$(BINDGEN_EXTRA_CLANG_ARGS)' $(CARGO_COMMAND) build --no-default-features $(FEATURES_ARG) --manifest-path bin/Cargo.toml $(TARGET_DOCKER_ARG)"

.PHONY:build-release
build-release: ## Build a release version of the agent
	$(UNCACHED_RUST_COMMAND) "$(BUILD_ENV_DOCKER_ARGS) --env RUST_BACKTRACE=full" "RUSTFLAGS='$(RUSTFLAGS)' BINDGEN_EXTRA_CLANG_ARGS='$(BINDGEN_EXTRA_CLANG_ARGS)' cargo build --no-default-features $(FEATURES_ARG) --manifest-path bin/Cargo.toml --release $(TARGET_DOCKER_ARG) && $(ARCH_TRIPLE)-strip ./target/$(TARGET)/release/logdna-agent"

.PHONY:check
check: ## Run unit tests
	$(RUST_COMMAND) "" "cargo check --all-targets"

.PHONY:test
test: test-journald ## Run unit tests
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "$(CARGO_COMMAND) test $(TARGET_DOCKER_ARG) --no-run && cargo test $(TARGET_DOCKER_ARG) $(TESTS)"

.PHONY:integration-test
integration-test: ## Run integration tests using image with additional tools
	$(eval FEATURES := $(FEATURES) integration_tests)
	$(DOCKER_JOURNALD_DISPATCH) "--env LOGDNA_INGESTION_KEY=$(LOGDNA_INGESTION_KEY) --env LOGDNA_HOST=$(LOGDNA_HOST) --env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "$(CARGO_COMMAND) test $(TARGET_DOCKER_ARG) $(FEATURES_ARG) --manifest-path bin/Cargo.toml $(TESTS) -- --nocapture --test-threads=$(INTEGRATION_TEST_THREADS)"

.PHONY:k8s-test
k8s-test: ## Run integration tests using k8s kind
	$(DOCKER_KIND_DISPATCH) $(K8S_TEST_CREATE_CLUSTER) $(RUST_IMAGE) "--env RUST_LOG=$(RUST_LOG)" "$(CARGO_COMMAND) test $(TARGET_DOCKER_ARG) --manifest-path bin/Cargo.toml --features k8s_tests -- --nocapture"

.PHONY:test-journald
test-journald: ## Run journald unit tests
	$(eval FEATURES := $(FEATURES) journald_tests)
	$(DOCKER_JOURNALD_DISPATCH) "--env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "$(CARGO_COMMAND) test $(TARGET_DOCKER_ARG) $(FEATURES_ARG) --manifest-path bin/Cargo.toml -p journald -- --nocapture --test-threads=1"

.PHONY:bench
bench:
	$(BENCH_COMMAND) "--privileged --env RUST_BACKTRACE=full --env RUST_LOG=$(RUST_LOG)" "PERF=\$$(find /usr/bin -type f -wholename /usr/bin/perf\* | head -n1) $(CARGO_COMMAND) run --release --manifest-path bench/Cargo.toml --bin=throughput /dict.txt -o /tmp/out $(PROFILE) --file-history 3 --line-count 100000000 --file-size 20000000 && mv /tmp/flamegraph.svg ."

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
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "$(CARGO_COMMAND) fmt -- --check"

.PHONY:lint-clippy
lint-clippy: ## Checks for code errors
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "$(CARGO_COMMAND) clippy --all-targets -- -D warnings"

.PHONY:lint-audit
lint-audit: ## Audits packages for issues
	$(RUST_COMMAND) "--env RUST_BACKTRACE=full" "$(CARGO_COMMAND) audit --ignore RUSTSEC-2020-0159 --ignore RUSTSEC-2020-0071"

.PHONY:lint-docker
lint-docker: ## Lint the Dockerfile for issues
	$(HADOLINT_COMMAND) "" "hadolint Dockerfile --ignore DL3006 --ignore SC2086"

.PHONY:lint-shell
lint-shell: ## Lint the Dockerfile for issues
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/lib.sh"
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/dispatch.sh"
	$(SHELLCHECK_COMMAND) "" "shellcheck docker/journald_dispatch.sh"

.PHONY:lint
lint: lint-docker lint-shell lint-format lint-clippy lint-audit ## Runs all the linters

.PHONY:bump-major-dev
bump-major-dev: ## Create a new minor beta release and push to github
	$(eval TARGET_BRANCH := $(shell expr $(MINOR_VERSION) + 1).0)
	$(eval NEW_VERSION := $(TARGET_BRANCH).0-dev)
	@if [ ! "$(REMOTE_BRANCH)" = "master" ]; then echo "Can't create the minor beta release \"$(NEW_VERSION)\" on the remote branch \"$(REMOTE_BRANCH)\". Please checkout \"master\""; exit 1; fi
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	git add bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git push

.PHONY:release-major
release-major: ## Create a new major beta release and push to github
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).0)
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

.PHONY:bump-minor-dev
bump-minor-dev: ## Create a new minor beta release and push to github
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(shell expr $(MINOR_VERSION) + 1))
	$(eval NEW_VERSION := $(TARGET_BRANCH).0-dev)
	@if [ ! "$(REMOTE_BRANCH)" = "master" ]; then echo "Can't create the minor beta release \"$(NEW_VERSION)\" on the remote branch \"$(REMOTE_BRANCH)\". Please checkout \"master\""; exit 1; fi
	$(call CHANGE_BIN_VERSION,$(NEW_VERSION))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_VERSION,$(NEW_VERSION),$(yaml))))
	$(foreach yaml,$(wildcard k8s/*.yaml),$(shell $(call CHANGE_K8S_IMAGE,$(NEW_VERSION),$(yaml))))
	git add bin/Cargo.toml
	git add -u k8s/
	git commit -sS -m "Bumping $(BUILD_VERSION) to $(NEW_VERSION)"
	git push

.PHONY:release-minor
release-minor: ## Create a new minor beta release and push to github
	$(eval TARGET_BRANCH := $(MAJOR_VERSION).$(MINOR_VERSION))
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

DEB_VERSION=1
DEB_ARCH_NAME_x86_64=amd64
DEB_ARCH_NAME_aarch64=arm64

.PHONY:build-image
build-image: ## Build a docker image as specified in the Dockerfile
	$(DOCKER) build . -t $(REPO):$(IMAGE_TAG) \
		$(PULL_OPTS) \
		--progress=plain \
		--platform=linux/${DEB_ARCH_NAME_${ARCH}} \
		--secret id=aws,src=$(AWS_SHARED_CREDENTIALS_FILE) \
		--rm \
		--build-arg BUILD_ENVS="$(BUILD_ENVS)" \
		--build-arg CARGO_COMMAND="$(CARGO_COMMAND)" \
		--build-arg BUILD_IMAGE=$(RUST_IMAGE) \
		--build-arg TARGET=$(TARGET) \
		--build-arg RUSTFLAGS='$(RUSTFLAGS)' \
		--build-arg BUILD_TIMESTAMP=$(BUILD_TIMESTAMP) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg FEATURES='$(FEATURES_ARG)' \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL) \
		--build-arg SCCACHE_BUCKET=$(SCCACHE_BUCKET) \
		--build-arg SCCACHE_REGION=$(SCCACHE_REGION) \
		--build-arg SCCACHE_ENDPOINT=$(SCCACHE_ENDPOINT)
	if [ ! -z "$(PULL_OPTS)" ]; then $(DOCKER) pull $(RUST_IMAGE); fi

.PHONY:build-image-debian
build-image-debian: ## Build a docker image as specified in the Dockerfile.debian
	$(DOCKER) build . -f Dockerfile.debian -t $(REPO):$(IMAGE_TAG) \
		$(PULL_OPTS) \
		--progress=plain \
		--secret id=aws,src=$(AWS_SHARED_CREDENTIALS_FILE) \
		--rm \
		--build-arg BUILD_ENVS="$(BUILD_ENVS)" \
		--build-arg BUILD_IMAGE=$(RUST_IMAGE) \
		--build-arg TARGET=$(TARGET) \
		--build-arg RUSTFLAGS='$(RUSTFLAGS)' \
		--build-arg BUILD_TIMESTAMP=$(BUILD_TIMESTAMP) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg FEATURES='$(FEATURES_ARG)' \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL) \
		--build-arg SCCACHE_BUCKET=$(SCCACHE_BUCKET) \
		--build-arg SCCACHE_REGION=$(SCCACHE_REGION) \
		--build-arg SCCACHE_ENDPOINT=$(SCCACHE_ENDPOINT)

.PHONY:build-image-debug
build-image-debug: ## Build a docker image as specified in the Dockerfile.debug
	$(DOCKER) build . -f Dockerfile.debug -t $(REPO):$(IMAGE_TAG) \
		$(PULL_OPTS) \
		--progress=plain \
		--secret id=aws,src=$(AWS_SHARED_CREDENTIALS_FILE) \
		--rm \
		--build-arg BUILD_ENVS="$(BUILD_ENVS)" \
		--build-arg BUILD_IMAGE=$(RUST_IMAGE) \
		--build-arg TARGET=$(TARGET) \
		--build-arg RUSTFLAGS='$(RUSTFLAGS)' \
		--build-arg BUILD_TIMESTAMP=$(BUILD_TIMESTAMP) \
		--build-arg BUILD_VERSION=$(BUILD_VERSION) \
		--build-arg FEATURES='$(FEATURES_ARG)' \
		--build-arg REPO=$(REPO) \
		--build-arg VCS_REF=$(VCS_REF) \
		--build-arg VCS_URL=$(VCS_URL) \
		--build-arg SCCACHE_BUCKET=$(SCCACHE_BUCKET) \
		--build-arg SCCACHE_REGION=$(SCCACHE_REGION) \
		--build-arg SCCACHE_ENDPOINT=$(SCCACHE_ENDPOINT)

.PHONY:build-deb
build-deb: build-release
	$(DEB_COMMAND) "" 'package_version="$(BUILD_VERSION)"; \
		iteration="${DEB_VERSION}"; \
		echo "Generating deb package for version ${BUILD_VERSION} as $${package_version}-$${iteration}"; \
		chmod +x "logdna-agent"; \
		fpm \
				-a "${ARCH}" \
				--input-type dir \
				--output-type deb \
				-p "/build/target/${TARGET}/logdna-agent_$${package_version}-$${iteration}_${DEB_ARCH_NAME_${ARCH}}.deb" \
				--name "logdna-agent" \
				--version "$${package_version}" \
				--iteration "$${iteration}" \
				--license MIT \
				--vendor "LogDNA, Inc." \
				--description "LogDNA Agent for Linux" \
				--url "https://logdna.com/" \
				--maintainer "LogDNA <support@logdna.com>" \
				--before-remove packaging/linux/before-remove \
				--after-upgrade packaging/linux/after-upgrade \
				--force --deb-no-default-config-files \
				"/build/target/${TARGET}/release/logdna-agent=/usr/bin/logdna-agent" \
				"packaging/linux/logdna-agent.service=/lib/systemd/system/logdna-agent.service"'

RPM_VERSION=1

.PHONY:build-rpm
build-rpm: build-release
	$(RPM_COMMAND) "" 'package_version="$(BUILD_VERSION)"; \
		iteration="${RPM_VERSION}"; \
		echo "Generating rpm package for version ${BUILD_VERSION} as $${package_version}-$${iteration}"; \
		chmod +x "logdna-agent"; \
		fpm \
				-a "${ARCH}" \
				--verbose \
				--input-type dir \
				--output-type rpm \
				-p "/build/target/${TARGET}/logdna-agent-$${package_version}-$${iteration}.${ARCH}.rpm" \
				--name "logdna-agent" \
				--version "$${package_version}" \
				--iteration "$${iteration}" \
				--license MIT \
				--vendor "LogDNA, Inc." \
				--description "LogDNA Agent for Linux" \
				--url "https://logdna.com/" \
				--maintainer "LogDNA <support@logdna.com>" \
				--before-remove packaging/linux/before-remove \
				--after-upgrade packaging/linux/after-upgrade \
				--force \
				"/build/target/${TARGET}/release/logdna-agent=/usr/bin/logdna-agent" \
				"packaging/linux/logdna-agent.service=/lib/systemd/system/logdna-agent.service"'

.PHONY: publish-s3-binary
publish-s3-binary:
	aws s3 cp --acl public-read target/$(TARGET)/release/logdna-agent s3://logdna-agent-build-bin/$(TARGET_TAG)/$(TARGET)/logdna-agent

define publish_images
	$(eval TARGET_VERSIONS := $(TARGET_TAG) $(shell if [ "$(BETA_VERSION)" = "0" ]; then echo "$(BUILD_VERSION)-$(BUILD_DATE).$(shell docker images -q $(REPO):$(IMAGE_TAG)) $(MAJOR_VERSION) $(MAJOR_VERSION).$(MINOR_VERSION)"; fi))
	@set -e; \
	arch=$(shell docker inspect --format "{{.Architecture}}" $(REPO):$(IMAGE_TAG)); \
	arr=($(TARGET_VERSIONS)); \
	for version in $${arr[@]}; do \
		echo "$(REPO):$(IMAGE_TAG) -> $(1):$${version}-$${arch}"; \
		$(DOCKER) tag $(REPO):$(IMAGE_TAG) $(1):$${version}-$${arch}; \
		$(DOCKER) push $(1):$${version}-$${arch}; \
	done;
endef

define publish_images_multi
	$(eval TARGET_VERSIONS := $(TARGET_TAG) $(shell if [ "$(BETA_VERSION)" = "0" ]; then echo "$(BUILD_VERSION)-$(BUILD_DATE).$(shell docker images -q $(REPO):$(IMAGE_TAG)) $(MAJOR_VERSION) $(MAJOR_VERSION).$(MINOR_VERSION)"; fi))
	@set -e; \
	arr=($(TARGET_VERSIONS)); \
	for version in $${arr[@]}; do \
		echo "$(REPO):$(IMAGE_TAG) -> $(1):$${version}"; \
		$(DOCKER) manifest create $(1):$${version} \
			--amend $(1):$${version}-arm64 \
			--amend $(1):$${version}-amd64; \
		$(DOCKER) manifest push $(1):$${version}; \
	done;
endef

.PHONY: publish-image
publish-image: publish-image-gcr publish-image-docker publish-image-ibm ## Publish SemVer compliant releases to our registries

.PHONY:publish-image-gcr
publish-image-gcr: ## Publish SemVer compliant releases to gcr
	$(call publish_images,$(DOCKER_PRIVATE_IMAGE))

.PHONY:publish-image-docker
publish-image-docker: ## Publish SemVer compliant releases to docker hub
	$(call publish_images,$(DOCKER_PUBLIC_IMAGE))

.PHONY:publish-image-ibm
publish-image-ibm: ## Publish SemVer compliant releases to icr
	$(call publish_images,$(DOCKER_IBM_IMAGE))

.PHONY: publish-image-multi
publish-image-multi: publish-image-multi-gcr publish-image-multi-docker publish-image-multi-ibm ## Publish multi-arch SemVer compliant releases to our registries

.PHONY:publish-image-multi-gcr
publish-image-multi-gcr: ## Publish multi-arch container images to gcr
	$(call publish_images_multi,$(DOCKER_PRIVATE_IMAGE))

.PHONY:publish-image-multi-docker
publish-image-multi-docker: ## Publish multi-arch container images to docker hub
	$(call publish_images_multi,$(DOCKER_PUBLIC_IMAGE))

.PHONY:publish-image-multi-ibm
publish-image-multi-ibm: ## Publish multi-arch container images to icr
	$(call publish_images_multi,$(DOCKER_IBM_IMAGE))

.PHONY:run
run: ## Run the debug version of the agent
	./target/debug/logdna-agent

.PHONY:run-release
run-release: ## Run the release version of the agent
	./target/release/logdna-agent

sysdig_secure_images: ## Create sysdig_secure_images config
	echo $(REPO):$(IMAGE_TAG) > sysdig_secure_images

.PHONY:help
help: ## Prints out a helpful description of each possible target
	@awk 'BEGIN {FS = ":.*?## "}; /^.+: .*?## / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@$(SHELL) -c "echo '$(TEST_RULES)'" |  awk 'BEGIN {FS = ":.*?<> "}; /^.+: .*?<> / && !/awk/ {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}'

.PHONY:init-qemu
init-qemu: ## register qemu in binfmt on x86_64 hosts
	@set -e
	echo "Host: " && hostname && uname -a && blkid && docker info && echo && free -h && echo && df -h && echo && lscpu && echo
	bash -c "if [ '$(shell uname -m)' = 'x86_64' ]; then \
		if [ ! -f /proc/sys/fs/binfmt_misc/qemu-aarch64 ]; then \
			( \
				flock 201; \
				docker run --rm --privileged multiarch/qemu-user-static --reset -p yes; \
			) 201>/tmp/qemu_binfmt; \
		else \
			echo Skipping qemu init - already applied; \
		fi; \
	else \
		echo Skipping qemu init - non x86_64 host; \
	fi"
