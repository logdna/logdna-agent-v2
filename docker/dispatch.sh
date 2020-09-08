#!/usr/bin/env sh

case "$(uname)" in
	Darwin*)	HOST_MACHINE=Mac;;
	*)		HOST_MACHINE=Linux;;
esac

if [ "$HOST_MACHINE" = "Mac" ]; then
	docker run --rm -w "$1" -v "$2" "$3" $4
elif [ "$HOST_MACHINE" = "Linux" ]; then
    docker run --rm -u $(id -u):$(id -g) -w "$1" -v "$2" "$3" $4
fi

