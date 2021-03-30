FROM docker.io/logdna/build-images:rust-buster-stable
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/cache/sccache

WORKDIR /work/
COPY . .

CMD [ "echo", "Starting..." ]
