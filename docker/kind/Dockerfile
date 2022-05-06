ARG BUILD_IMAGE docker.io/logdna/build-images:rust-bullseye-1-stable-x86_64
FROM ${BUILD_IMAGE}
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/cache/sccache

WORKDIR /work/
COPY . .

CMD [ "echo", "Starting..." ]
