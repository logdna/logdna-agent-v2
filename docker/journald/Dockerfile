FROM docker.io/logdna/build-images:rust-buster-stable
RUN apt update -y && apt install -y systemd
STOPSIGNAL SIGRTMIN+3

RUN mkdir -p /var/log/journal

WORKDIR /work/
COPY . .

CMD [ "/bin/systemd", "--system", "--unit=basic.target" ]
