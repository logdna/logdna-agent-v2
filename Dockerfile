# select build image
FROM rust:1.42 as build

ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

COPY . /agent
WORKDIR /agent

RUN cargo build --release
RUN strip target/release/logdna-agent

# our final base
FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive
ENV _RJEM_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="narenas:1,tcache:false,dirty_decay_ms:0,muzzy_decay_ms:0"

RUN apt update -y && \
apt upgrade -y --fix-missing && \
apt install ca-certificates -y

# copy the build artifact from the build stage
COPY --from=build /agent/target/release/logdna-agent /work/
WORKDIR /work/
RUN chmod -R 777 .

CMD ["./logdna-agent"]
