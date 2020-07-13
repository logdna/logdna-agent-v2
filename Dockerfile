# select build image
FROM rust:1.41 as build

ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

# Create the directory for agent repo
WORKDIR /opt/logdna-agent-v2

# Only add dependency lists first for caching
COPY Cargo.lock                     .
COPY Cargo.toml                     .
COPY bin/Cargo.toml                 bin/Cargo.toml
COPY common/config-macro/Cargo.toml common/config-macro/
COPY common/config/Cargo.toml       common/config/
COPY common/fs/Cargo.toml           common/fs/
COPY common/http/Cargo.toml         common/http/
COPY common/journald/Cargo.toml     common/journald/
COPY common/k8s/Cargo.toml          common/k8s/
COPY common/metrics/Cargo.toml      common/metrics/
COPY common/middleware/Cargo.toml   common/middleware/
COPY common/source/Cargo.toml       common/source/

# Create required scaffolding for dependencies
RUN LIBDIRS=$(find . -name Cargo.toml -type f -mindepth 2 | sed 's/Cargo.toml//') \
    && for LIBDIR in ${LIBDIRS}; do \
        mkdir ${LIBDIR}/src \
        && touch ${LIBDIR}/src/lib.rs; \
    done \
    && echo "fn main() {}" > bin/src/main.rs

# Build cached dependencies
RUN cargo build --release

# Delete all cached deps that are local libs
RUN grep -aL "github.com" target/release/deps/* | xargs rm \
  && rm target/release/deps/libconfig_macro*.so

# Add the actual agent source files
COPY . .

# Rebuild the agent
RUN cargo build --release
RUN strip target/release/logdna-agent

# Use ubuntu as the final base image
FROM ubuntu:18.04

ARG REPO
ARG BUILD_DATE
ARG VCS_REF
ARG VCS_URL
ARG BUILD_VERSION

LABEL org.label-schema.schema-version="1.0"
LABEL org.label-schema.url="https://logdna.com"
LABEL org.label-schema.build-date="${BUILD_DATE}"
LABEL org.label-schema.maintainer="LogDNA <support@logdna.com>"
LABEL org.label-schema.name="answerbook/${REPO}"
LABEL org.label-schema.description="LogDNA Agent V2"
LABEL org.label-schema.vcs-url="${VCS_URL}"
LABEL org.label-schema.vcs-ref="${VCS_REF}"
LABEL org.label-schema.vendor="LogDNA Inc."
LABEL org.label-schema.version="${BUILD_VERSION}"
LABEL org.label-schema.docker.cmd="docker run logdna-agent-v2"

ENV DEBIAN_FRONTEND=noninteractive
ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

RUN apt update -y && \
apt upgrade -y --fix-missing && \
apt install ca-certificates -y

# Copy the agent binary from the build stage
COPY --from=build /opt/logdna-agent-v2/target/release/logdna-agent /work/
WORKDIR /work/
RUN chmod -R 777 .

CMD ["./logdna-agent"]
