#!/usr/bin/env sh

curpath=$(dirname "$0")
image="logdna-agent-journald:latest"

# shellcheck source=/dev/null
. "$curpath/lib.sh"

docker build --tag "$image" --build-arg "UID=$(id -u)" --build-arg "BUILD_IMAGE=$BUILD_IMAGE" --build-arg "ARCH=$ARCH" --file "$curpath/journald/Dockerfile" "$curpath/.."

journald_args="--tmpfs /tmp --tmpfs /run --tmpfs /var/log/journal"

extra_args="$(get_volume_mounts "$1" "$image") $(get_sccache_args) $journald_args"

if [ "$HOST_MACHINE" = "Mac" ]; then
  # shellcheck disable=SC2086
  docker run --rm --tty --workdir "$1" $extra_args --volume "$2" $3 "$image" /bin/sh -c "$4"
elif [ "$HOST_MACHINE" = "Linux" ]; then
  # shellcheck disable=SC2086,SC2154
  docker run --rm --tty --workdir "$1" $extra_args --volume "$2" $3 --user "$(id -u)":"$(id -g)" "$image" /bin/sh -c "$start_sccache; $4"
fi
