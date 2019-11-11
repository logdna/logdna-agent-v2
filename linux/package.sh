#!/bin/bash

export APP='logdna-agent'
export VENDOR='logdna.com'

export SCRIPT_DIR=$(dirname "$0")
export PROJECT_DIR=$(cd $SCRIPT_DIR && cd ../ && pwd)

export CARGO_VERSION=$(grep -m 1 "version = \"[0-9a-z.-]*\"" ${PROJECT_DIR}/bin/Cargo.toml | sed -n 's/version = "\([0-9a-z.-]*\)"/\1/p')
export GIT_VERSION=$(git rev-parse --short HEAD)

## AMD64 binary

OS="linux"
SYSTEMD_DIR="/etc/systemd/system/"
PKG_INSTALL_BIN_DIR="/usr/bin/"
PKG_NAME="$APP"
OUTPUT_DIR="${PROJECT_DIR}/target/pkg/$ARCH"
AGENT_SYSTEMD_SERVICE="${SCRIPT_DIR}/logdna-agent.service"
POST_INSTALL_SCRIPT="${SCRIPT_DIR}/postinst"

DEB_VERSION="$CARGO_VERSION"
RPM_VERSION="$CARGO_VERSION"

if [ -d "$OUTPUT_DIR" ]; then
  echo "$OUTPUT_DIR found, removing old artifacts"
  rm -rf $OUTPUT_DIR
fi

mkdir -p $OUTPUT_DIR

cross build --release --target x86_64-unknown-linux-gnu

ARCH="x86_64"
BUILD_BIN_FILE="${PROJECT_DIR}/target/x86_64-unknown-linux-gnu/release/logdna-agent"

fpm \
  -s dir \
  -t deb \
  -n $PKG_NAME \
  -p $OUTPUT_DIR \
  -v $DEB_VERSION \
  -a $ARCH \
  --vendor $VENDOR \
  --after-install $POST_INSTALL_SCRIPT \
  $BUILD_BIN_FILE=$PKG_INSTALL_BIN_DIR \
  $AGENT_SYSTEMD_SERVICE=$SYSTEMD_DIR

fpm \
  -s dir \
  -t rpm \
  -n $PKG_NAME \
  -p $OUTPUT_DIR \
  -v $RPM_VERSION \
  -a $ARCH \
  --vendor $VENDOR \
  --after-install $POST_INSTALL_SCRIPT \
  $BUILD_BIN_FILE=$PKG_INSTALL_BIN_DIR \
  $AGENT_SYSTEMD_SERVICE=$SYSTEMD_DIR

ARCH="aarch64"
BUILD_BIN_FILE="${PROJECT_DIR}/target/aarch64-unknown-linux-gnu/release/logdna-agent"

cross build --release --target aarch64-unknown-linux-gnu

fpm \
  -s dir \
  -t deb \
  -n $PKG_NAME \
  -p $OUTPUT_DIR \
  -v $DEB_VERSION \
  -a $ARCH \
  --vendor $VENDOR \
  --after-install $POST_INSTALL_SCRIPT \
  $BUILD_BIN_FILE=$PKG_INSTALL_BIN_DIR \
  $AGENT_SYSTEMD_SERVICE=$SYSTEMD_DIR

fpm \
  -s dir \
  -t rpm \
  -n $PKG_NAME \
  -p $OUTPUT_DIR \
  -v $DEB_VERSION \
  -a $ARCH \
  --vendor $VENDOR \
  --after-install $POST_INSTALL_SCRIPT \
  $BUILD_BIN_FILE=$PKG_INSTALL_BIN_DIR \
  $AGENT_SYSTEMD_SERVICE=$SYSTEMD_DIR
