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

	if [ "$HOST_MACHINE" = "Mac" ]; then
		# host mounts are hideously slow on Mac, create docker volumes instead
		docker volume create cargo_cache > /dev/null
		docker volume create agent-v2-cache > /dev/null
		echo "-v cargo_cache:$cargo_home/registry -v agent-v2-cache:$1/target"
	elif [ "$HOST_MACHINE" = "Linux" ]; then
		mkdir -p "$CARGO_CACHE/git" "$CARGO_CACHE/registry"
		echo "-v $CARGO_CACHE/git:$cargo_home/git:Z -v $CARGO_CACHE/registry:$cargo_home/registry:Z"
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
		echo "--env SCCACHE_BUCKET=$SCCACHE_BUCKET --env SCCACHE_REGION=$SCCACHE_REGION --env AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY --env AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID"
	fi
}
