FROM docker.io/logdna/build-images:rust-buster-1-stable-x86_64
RUN apt update -y && apt install -y systemd lsof procps
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/log/journal

WORKDIR /work/
COPY . .

CMD [ "/bin/systemd", "--system", "--unit=basic.target" ]
