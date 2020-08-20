#!/bin/sh

USER=$(id -nu "$1" 2>&1)
GROUP=$(getent group "$2" | cut -d: -f1)

if [ -z "$GROUP" ]; then
    GROUP=logdna
    groupadd -g "$2" logdna
fi

if case "$USER" in *"no such user") true ;; *) false ;; esac; then
    USER=logdna
    useradd -g "$GROUP" -u "$1" -m logdna
fi

#apt-get update -y
#apt-get install -y strace

chown -R "$USER:$GROUP" "$3"
runuser -u "$USER" -g "$GROUP" -- /bin/sh -c "$4"
