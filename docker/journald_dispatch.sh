#!/usr/bin/env sh

curpath=$(dirname "$0")
image="logdna-agent-journald:latest"

_term() {
	if [ -z "$child" ]; then
    	status=$?
    else
		docker kill "$child"
		status=$(docker inspect "$child" --format='{{.State.ExitCode}}')
		docker rm "$child"
    fi
	docker rmi "$image"
	exit "$status"
}

# shellcheck source=/dev/null
. "$curpath/lib.sh"

docker build -t "$image" --build-arg "UID=$(id -u)" --build-arg "BUILD_IMAGE=$BUILD_IMAGE" --build-arg "ARCH=$ARCH" -f "$curpath/journald/Dockerfile" "$curpath/.."

trap _term TERM
trap _term INT

journald_args="--tmpfs /tmp --tmpfs /run --tmpfs /run/lock"

# Handle cgroups v2 and v1
if [ -f "/sys/fs/cgroup/cgroup.controllers" ]; then # the kernel has cgroup v2 enabled
  journald_args="$journald_args --privileged --cap-add SYS_ADMIN --security-opt seccomp=unconfined --cgroup-parent=docker.slice --cgroupns private"
else
  journald_args="$journald_args -v /sys/fs/cgroup:/sys/fs/cgroup:ro"

fi

extra_args="$(get_volume_mounts "$1" "$image") $(get_sccache_args) $journald_args"

# shellcheck disable=SC2086
child=$(docker run -d -w "$1" $extra_args -v "$2" $3 "$image")

if [ "$HOST_MACHINE" = "Mac" ]; then
	docker exec "$child" /bin/sh -c "$4"
elif [ "$HOST_MACHINE" = "Linux" ]; then
	docker exec -u "$(id -u)":"$(id -g)" "$child" /bin/sh -c "$4"
fi

status=$?

# Clean up the container
docker kill "$child" > /dev/null
docker rm "$child" > /dev/null
docker rmi "$image" > /dev/null

exit "$status"
