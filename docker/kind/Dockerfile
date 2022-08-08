FROM docker.io/logdna/build-images:rust-buster-1-stable-x86_64
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/cache/sccache

WORKDIR /work/
COPY . .

CMD [ "echo", "Starting..." ]
