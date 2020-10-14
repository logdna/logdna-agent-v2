#!/usr/bin/env sh

_term() {
  docker kill "$child"
  status=$(docker inspect "$child" --format='{{.State.ExitCode}}')
  docker rm "$child"
  exit "$status"
}

case "$(uname)" in
	Darwin*)	HOST_MACHINE=Mac;;
	*)		HOST_MACHINE=Linux;;
esac

# Get the container's CARGO_HOME ENV, defaulting to /usr/local/cargo
cargo_home=$(docker inspect -f \
        '{{range $index, $value := .Config.Env}}{{println $value}}{{end}}' \
            "$3" | grep CARGO_HOME | cut -d"=" -f 2)
if [ -z "$cargo_home" ]; then
	cargo_home="/usr/local/cargo"
fi

volume_mounts=""

if [ "$HOST_MACHINE" = "Mac" ]; then
	# host mounts are hideously slow on Mac, create docker volumes instead
	docker volume create cargo_cache > /dev/null
	docker volume create agent-v2-cache > /dev/null
	volume_mounts="$volume_mounts -v cargo_cache:$cargo_home/registry -v agent-v2-cache:$1/target"
elif [ "$HOST_MACHINE" = "Linux" ]; then
	mkdir -p "$CARGO_CACHE/git" "$CARGO_CACHE/registry"
	volume_mounts="$volume_mounts -v $CARGO_CACHE/git:$cargo_home/git:Z -v $CARGO_CACHE/registry:$cargo_home/registry:Z"
fi

get_sccache_args()
{
	if [ -z "$SCCACHE_BUCKET" ]; then
		sccache_dir=""
		if [ -z "$sccache_dir" ]; then
			if [ "$HOST_MACHINE" = "Mac" ]; then
				# host mounts are hideously slow on Mac, create docker volumes instead
				sccache_dir=sccache_volume
				docker volume create $sccache_dir > /dev/null
			elif [ "$HOST_MACHINE" = "Linux" ]; then
				sccache_dir=~/.cache/sccache
				mkdir -p $sccache_dir
			fi
		fi
		echo "-v $sccache_dir:/var/cache/sccache:Z --env SCCACHE_DIR=/var/cache/sccache"
	else
		echo "--env SCCACHE_BUCKET=$SCCACHE_BUCKET --env SCCACHE_REGION=$SCCACHE_REGION --env AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY --env AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID"
	fi
}

extra_args="$volume_mounts $(get_sccache_args)"

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
