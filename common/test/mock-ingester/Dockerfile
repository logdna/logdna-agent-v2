# select build image
FROM rust:1.42 as build

COPY . /mock-ingester
WORKDIR /mock-ingester

RUN cargo build --release

# our final base
FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt update -y && \
apt upgrade -y --fix-missing && \
apt install ca-certificates -y

# copy the build artifact from the build stage
COPY --from=build /mock-ingester/target/release/mock-ingester /mock-ingester/
WORKDIR /mock-ingester/
RUN chmod -R 777 .

CMD ["./mock-ingester"]
