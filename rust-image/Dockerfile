ARG BUILD_IMAGE

FROM ${BUILD_IMAGE}-slim as build

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

RUN apt-get update && apt-get install -y \
  make \
  libssl-dev \
  pkg-config \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/cargo/bin /usr/local/cargo/bin
COPY --from=build /usr/local/rustup/toolchains /usr/local/rustup/toolchains
