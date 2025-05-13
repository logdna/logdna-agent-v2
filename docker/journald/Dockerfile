ARG BUILD_IMAGE

FROM ${BUILD_IMAGE}

ARG UID

RUN <<-EOF
  apt-get update
  apt-get install -y systemd systemd-journald libsystemd0
  apt-get clean
  rm -rf /var/lib/apt/lists/*
EOF

RUN <<-EOF
  mkdir -p /var/log/journal /run/systemd/journal/docket
  touch /run/systemd/journal/socket
EOF

RUN echo "uninitialized" > /etc/machine-id || true

RUN <<-EOF
  useradd -u ${UID} -G systemd-journal test_user
  chown :systemd-journal /var/log/journal
  chmod g+s /var/log/journal
EOF


WORKDIR /work/

CMD [ "/lib/systemd/systemd-journald" ]
