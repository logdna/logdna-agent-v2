#!/usr/bin/env sh

case "$(uname)" in
	Darwin*)	HOST_MACHINE=Mac;;
	*)			HOST_MACHINE=Linux;;
esac

get_volume_mounts() {
	# Get the container's CARGO_HOME ENV, defaulting to /usr/local/cargo
	cargo_home=$(docker inspect -f \
	'{{range $index, $value := .Config.Env}}{{println $value}}{{end}}' \
	"$2" | grep CARGO_HOME | cut -d"=" -f 2)
	if [ -z "$cargo_home" ]; then
		cargo_home="/usr/local/cargo"
	fi
	if [ "$HOST_MACHINE" = "Mac" ] && [ "$CACHE_TARGET" != "false" ]; then
		docker volume create agent-v2-cache > /dev/null
		echo " -v agent-v2-cache:$1/target"
	fi
	if [ "$HOST_MACHINE" = "Mac" ]; then
		# host mounts are hideously slow on Mac, create docker volumes instead
		docker volume create cargo_cache > /dev/null
		echo "-v cargo_cache:$cargo_home/registry"
		docker volume create cargo_xwin_cache > /dev/null
		echo "-v cargo_xwin_cache:$cargo_home/xwin"
	elif [ "$HOST_MACHINE" = "Linux" ]; then
		mkdir -p "$CARGO_CACHE/git" "$CARGO_CACHE/registry" "$CARGO_CACHE/xwin"
		echo "-v $CARGO_CACHE/git:$cargo_home/git:Z -v $CARGO_CACHE/registry:$cargo_home/registry:Z -v $CARGO_CACHE/xwin:$cargo_home/xwin:Z"
	fi
}

get_sccache_args() {
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
		if [ -n "$SCCACHE_ENDPOINT" ]; then APPEND="--env SCCACHE_ENDPOINT=$SCCACHE_ENDPOINT"; fi
		echo "--env SCCACHE_BUCKET=$SCCACHE_BUCKET --env SCCACHE_REGION=$SCCACHE_REGION --env AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY --env AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID $APPEND"
	fi
}

get_host_arch() {
	# Default to the host ARCH
	HOST_ARCH=$(uname -m)
	# special case handling for M1s
	if [ "$HOST_ARCH" = 'arm64' ]; then
		HOST_ARCH=${HOST_ARCH:-'aarch64'}
	else
		HOST_ARCH=${HOST_ARCH:-$HOST_ARCH}
	fi
	echo "$HOST_ARCH"
}

# shellcheck disable=SC2034
start_sccache="if [ ! -z \${RUSTC_WRAPPER+x} ]; then while sccache --start-server > /dev/null 2>&1; do echo 'starting sccache server'; done; fi"
