ARG ARCH=x86_64
FROM docker.io/logdna/build-images:rust-buster-1-stable-${ARCH}
STOPSIGNAL SIGRTMIN+3

RUN rm -f /lib/systemd/system/multi-user.target.wants/* \
  /etc/systemd/system/*.wants/* \
  /lib/systemd/system/local-fs.target.wants/* \
  /lib/systemd/system/sockets.target.wants/*udev* \
  /lib/systemd/system/sockets.target.wants/*initctl* \
  /lib/systemd/system/sysinit.target.wants/systemd-tmpfiles-setup* \
  /lib/systemd/system/systemd-update-utmp*

RUN mkdir -p /var/log/journal

WORKDIR /work/
COPY . .

RUN chmod 777 /etc
RUN mkdir /etc/logdna
RUN chmod 777 /etc/logdna

CMD [ "/bin/systemd", "--system", "--unit=basic.target" ]
