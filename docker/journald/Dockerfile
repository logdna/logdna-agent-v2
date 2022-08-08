FROM docker.io/logdna/build-images:rust-buster-1-stable-x86_64
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/log/journal

WORKDIR /work/
COPY . .

CMD [ "/bin/systemd", "--system", "--unit=basic.target" ]
