# select build image
FROM rust:1.38 as build

COPY . /agent
WORKDIR /agent

RUN cargo build --release
RUN strip target/release/logdna-agent

# our final base
FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt update -y && \
apt upgrade -y --fix-missing && \
apt install ca-certificates -y

# copy the build artifact from the build stage
COPY --from=build /agent/target/release/logdna-agent /work/
WORKDIR /work/
RUN chmod -R 777 .

CMD ["./logdna-agent"]