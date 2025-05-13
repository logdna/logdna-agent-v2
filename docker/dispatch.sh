#!/usr/bin/env sh

curpath=$(dirname "$0")

# shellcheck source=/dev/null
. "$curpath/lib.sh"

extra_args="$(get_volume_mounts "$1" "$3") $(get_sccache_args)"

if [ "$HOST_MACHINE" = "Mac" ]; then
  # shellcheck disable=SC2086
  docker run --rm --tty --workdir "$1" $extra_args --volume "$2" $4 "$3" /bin/sh -ic "$5"
elif [ "$HOST_MACHINE" = "Linux" ]; then
  # shellcheck disable=SC2086,SC2154
  docker run --rm --tty --user "$(id -u)":"$(id -g)" --workdir "$1" $extra_args --volume "$2" $4 "$3" /bin/sh -ic "$start_sccache; $5"
else
  echo "Error: Unknown or incompatible OS detected!"
  exit 1
fi

