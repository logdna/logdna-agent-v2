# syntax = docker/dockerfile:1.0-experimental

ARG UBI_VERSION=8.5
ARG TARGET_ARCH=x86_64
ARG BUILD_IMAGE
# Image that runs natively on the BUILDPLATFORM to produce cross compile
# artifacts

FROM --platform=${TARGETPLATFORM} registry.access.redhat.com/ubi8/ubi-minimal:${UBI_VERSION} as target

FROM --platform=${BUILDPLATFORM} ${BUILD_IMAGE} as build

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ARG TARGET_ARCH
ARG TARGET

ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

ARG FEATURES

ARG SCCACHE_BUCKET
ARG SCCACHE_REGION
ARG SCCACHE_ENDPOINT
ARG SCCACHE_SERVER_PORT=4226
ARG SCCACHE_RECACHE
ARG AWS_ACCESS_KEY_ID
ARG AWS_SECRET_ACCESS_KEY

ARG BUILD_ENVS

ARG TARGET

ENV SYSROOT_PATH="/sysroot/ubi-${UBI_VERSION}"

ENV RUST_LOG=rustc_codegen_ssa::back::link=info

# Create the directory for agent repo
WORKDIR /opt/logdna-agent-v2

# Install the target image libraries we want to link against.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install --no-install-recommends -y dnf
COPY --from=target /etc/yum.repos.d/ubi.repo /etc/yum.repos.d/ubi.repo
COPY --from=target /etc/pki/rpm-gpg/RPM-GPG-KEY-redhat-release /etc/pki/rpm-gpg/RPM-GPG-KEY-redhat-release
ENV UBI_PACKAGES="systemd-libs systemd-devel glibc glibc-devel gcc libstdc++-devel libstdc++-static kernel-headers"
RUN dnf install --releasever=8 --forcearch="${TARGET_ARCH}" \
        --installroot=$SYSROOT_PATH/ --repo=ubi-8-baseos --repo=ubi-8-appstream \
        --repo=ubi-8-codeready-builder -y $UBI_PACKAGES

RUN printf "/* GNU ld script\n*/\n\
OUTPUT_FORMAT(elf64-%s)\n\
GROUP ( /usr/lib64/libgcc_s.so.1  AS_NEEDED ( /usr/lib64/libgcc_s.so.1 ) )" "$(echo ${TARGET_ARCH} | tr '_' '-' )" > $SYSROOT_PATH/usr/lib64/libgcc_s.so

# Add the actual agent source files
COPY . .

# Set up env vars so that the compilers know to link against the target image libraries rather than the base image's
ENV LD_LIBRARY_PATH="-L $SYSROOT_PATH/usr/lib/gcc/${TARGET_ARCH}-redhat-linux/8/ -L $SYSROOT_PATH/usr/lib64"

ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-Clink-arg=--sysroot=$SYSROOT_PATH -Clink-arg=-fuse-ld=lld -Clink-arg=--target=x86_64-unknown-linux-gnu"
ENV CFLAGS_x86_64_unknown_linux_gnu="${CFLAGS_x86_64_unknown_linux_gnu} --sysroot $SYSROOT_PATH -isysroot=$SYSROOT_PATH ${LD_LIBRARY_PATH}"
ENV CXXFLAGS_x86_64_unknown_linux_gnu="${CXXFLAGS_x86_64_unknown_linux_gnu} --sysroot $SYSROOT_PATH -isysroot=$SYSROOT_PATH"

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-Clink-arg=--sysroot=$SYSROOT_PATH -Clink-arg=-fuse-ld=lld -Clink-arg=--target=aarch64-unknown-linux-gnu"
ENV CFLAGS_aarch64_unknown_linux_gnu="${CFLAGS_aarch64_unknown_linux_gnu} --sysroot $SYSROOT_PATH -isysroot=$SYSROOT_PATH ${LD_LIBRARY_PATH}"
ENV CXXFLAGS_aarch64_unknown_linux_gnu="${CXXFLAGS_aarch64_unknown_linux_gnu} --sysroot $SYSROOT_PATH -isysroot=$SYSROOT_PATH"

ENV LDFLAGS="-fuse-ld=lld"
ENV SYSTEMD_LIB_DIR="$SYSROOT_PATH/lib64"

ENV TARGET_CFLAGS=CFLAGS_${TARGET_ARCH}_unknown_linux_gnu
ENV TARGET_CXXFLAGS=CXXFLAGS_${TARGET_ARCH}_unknown_linux_gnu

RUN env

# Rebuild the agent
RUN --mount=type=secret,id=aws,target=/root/.aws/credentials \
    --mount=type=cache,target=/opt/rust/cargo/registry  \
    --mount=type=cache,target=/opt/logdna-agent-v2/target \
    if [ -z "$SCCACHE_BUCKET" ]; then unset RUSTC_WRAPPER; fi; \
    if [ -n "${TARGET}" ]; then export TARGET_ARG="--target ${TARGET}"; fi; \
    export ${BUILD_ENVS?};  \
    if [ -z "$SCCACHE_ENDPOINT" ]; then unset SCCACHE_ENDPOINT; fi; \
    export EXTRA_CFLAGS=${!TARGET_CFLAGS}; \
    export EXTRA_CXXFLAGS=${!TARGET_CXXFLAGS}; \
    export BINDGEN_EXTRA_CLANG_ARGS="${!TARGET_CXXFLAGS}"; \
    cargo build --manifest-path bin/Cargo.toml --no-default-features ${FEATURES} --release $TARGET_ARG && \
    llvm-strip ./target/${TARGET}/release/logdna-agent && \
    cp ./target/${TARGET}/release/logdna-agent /logdna-agent && \
    sccache --show-stats


ARG UBI_VERSION

# Use Red Hat Universal Base Image Minimal as the final base image
FROM --platform=${TARGETPLATFORM} registry.access.redhat.com/ubi8/ubi-minimal:${UBI_VERSION}

ARG REPO
ARG BUILD_TIMESTAMP
ARG VCS_REF
ARG VCS_URL
ARG BUILD_VERSION

LABEL org.opencontainers.image.created="${BUILD_TIMESTAMP}"
LABEL org.opencontainers.image.authors="LogDNA <support@logdna.com>"
LABEL org.opencontainers.image.url="https://logdna.com"
LABEL org.opencontainers.image.documentation=""
LABEL org.opencontainers.image.source="${VCS_URL}"
LABEL org.opencontainers.image.version="${BUILD_VERSION}"
LABEL org.opencontainers.image.revision="${VCS_REF}"
LABEL org.opencontainers.image.vendor="LogDNA Inc."
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.ref.name=""
LABEL org.opencontainers.image.title="LogDNA Agent"
LABEL org.opencontainers.image.description="The blazingly fast, resource efficient log collection client"

ENV DEBIAN_FRONTEND=noninteractive
ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

# Copy the agent binary from the build stage
COPY --from=build /logdna-agent /work/
WORKDIR /work/

RUN microdnf update -y \
    && microdnf install ca-certificates libcap shadow-utils -y \
    && rm -rf /var/cache/yum \
    && chmod -R 777 . \
    && setcap "cap_dac_read_search+eip" /work/logdna-agent \
    && groupadd -g 5000 logdna \
    && useradd -u 5000 -g logdna logdna

CMD ["./logdna-agent"]
