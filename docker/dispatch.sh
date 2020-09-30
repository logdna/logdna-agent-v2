#!/usr/bin/env sh

curpath=$(dirname "$0")

_term() {
  docker kill "$child"
  status=$(docker inspect "$child" --format='{{.State.ExitCode}}')
  docker rm "$child"
  exit "$status"
}

# shellcheck source=/dev/null
. "$curpath/lib.sh"

extra_args="$(get_volume_mounts "$1" "$3") $(get_sccache_args)"

trap _term TERM
trap _term INT

if [ "$HOST_MACHINE" = "Mac" ]; then
	# shellcheck disable=SC2086
	child=$(docker run -d -w "$1" $extra_args -v "$2" $4 "$3" /bin/sh -c "$5")
elif [ "$HOST_MACHINE" = "Linux" ]; then
	# shellcheck disable=SC2086
	child=$(docker run -d -u "$(id -u)":"$(id -g)" -w "$1" $extra_args -v "$2" $4 "$3" /bin/sh -c "$5")
fi

# Tail the container til it's done
docker logs -f "$child"

# Get the exit code of completed container
status=$(docker inspect "$child" --format='{{.State.ExitCode}}')

# Clean up the container
docker rm "$child" > /dev/null

exit "$status"
