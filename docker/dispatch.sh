#!/usr/bin/env sh

curpath=$(dirname "$0")

_term() {
  # disable=sc2317
  docker kill "$child"
  # disable=sc2317
  status=$(docker inspect "$child" --format='{{.State.ExitCode}}')
  # disable=sc2317
  docker rm "$child"
  # disable=sc2317
  exit "$status"
}

# shellcheck source=/dev/null
. "$curpath/lib.sh"

extra_args="$(get_volume_mounts "$1" "$3") $(get_sccache_args)"

trap _term TERM
trap _term INT

if [ "$HOST_MACHINE" = "Mac" ]; then
	# shellcheck disable=SC2086
	child=$(docker run -dit -w "$1" $extra_args -v "$2" $4 "$3" /bin/sh -ic "$5")
	#docker run --rm -it -w "$1" $extra_args -v "$2" $4 "$3" /bin/bash -ic "$5"
elif [ "$HOST_MACHINE" = "Linux" ]; then
	# shellcheck disable=SC2086
	child=$(docker run -dit -u "$(id -u)":"$(id -g)" -w "$1" $extra_args -v "$2" $4 "$3" /bin/sh -ic "$5")
fi

# Tail the container til it's done
docker logs -f "$child"

# Get the exit code of completed container
status=$(docker inspect "$child" --format='{{.State.ExitCode}}')

# Clean up the container
docker rm "$child" > /dev/null

exit "$status"
