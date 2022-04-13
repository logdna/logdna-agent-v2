ARG BUILD_IMAGE

FROM ${BUILD_IMAGE}

ARG UID

STOPSIGNAL SIGRTMIN+3

RUN rm -f /lib/systemd/system/multi-user.target.wants/* \
  /etc/systemd/system/*.wants/* \
  /lib/systemd/system/local-fs.target.wants/* \
  /lib/systemd/system/sockets.target.wants/*udev* \
  /lib/systemd/system/sockets.target.wants/*initctl* \
  /lib/systemd/system/sysinit.target.wants/systemd-tmpfiles-setup* \
  /lib/systemd/system/systemd-update-utmp*

RUN mkdir -p /var/log/journal && \
    useradd -u ${UID} -G systemd-journal test_user && \
    chown :systemd-journal /var/log/journal && \
    chmod g+s /var/log/journal


WORKDIR /work/

RUN chmod 777 /etc
RUN mkdir /etc/logdna
RUN chmod 777 /etc/logdna

CMD [ "/bin/systemd", "--system", "--unit=basic.target" ]
