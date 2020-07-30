ARG BUILD_IMAGE

FROM ${BUILD_IMAGE}-slim as build

ARG REPO
ARG VCS_REF

LABEL com.logdna.stage="${REPO}-build"
LABEL com.logdna.tag="${VCS_REF}"

ARG REPO
ARG VCS_REF

WORKDIR /opt/rust-image

COPY Makefile .

RUN apt-get update && apt-get install -y \
  make \
  libssl-dev \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

RUN make test-deps

FROM ${BUILD_IMAGE}-slim

ARG REPO
ARG VCS_REF

LABEL com.logdna.stage="${REPO}-final"
LABEL com.logdna.tag="${VCS_REF}"

RUN apt-get update && apt-get install -y \
  make \
  libssl-dev \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/cargo/bin /usr/local/cargo/bin
COPY --from=build /usr/local/rustup/toolchains /usr/local/rustup/toolchains
