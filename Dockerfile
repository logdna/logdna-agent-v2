# select build image
FROM rust:1.36 as build

COPY . /agent
WORKDIR /agent

RUN cargo build --release
RUN strip target/release/logdna-agent

# our final base
FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt update -y && \
apt upgrade -y --fix-missing && \
apt install ca-certificates -y && \
apt install -y curl && \
apt install -y gnupg2 && \
apt install -y apt-transport-https && \
curl -s https://packages.cloud.google.com/apt/doc/apt-key.gpg | apt-key add - && \
echo "deb https://apt.kubernetes.io/ kubernetes-xenial main" | tee -a /etc/apt/sources.list.d/kubernetes.list && \
apt update -y && \
apt install -y kubectl

# copy the build artifact from the build stage
COPY --from=build /agent/target/release/logdna-agent /work/
WORKDIR /work/
RUN chmod -R 777 .

CMD ["./logdna-agent"]