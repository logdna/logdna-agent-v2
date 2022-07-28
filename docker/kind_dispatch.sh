#!/usr/bin/env sh

set -e

curpath=$(realpath $(dirname "$0"))

if [ -z "$BUILD_TAG" ]
then
  cluster_name=agent-dev-cluster
else
  cluster_name=$(echo $BUILD_TAG | tr '[:upper:]' '[:lower:]' | tail -c 32 | sed 's/^-*//g')

fi

kind_network=$($curpath/kind_start.sh $3 $cluster_name)

# shellcheck source=/dev/null
. "$curpath/lib.sh"

extra_args="$(get_volume_mounts "$1" "$4") $(get_sccache_args)"

_term() {
  docker kill "$child"
  status=$(docker inspect "$child" --format='{{.State.ExitCode}}')
  docker rm "$child"
  $curpath/kind_stop.sh
  exit "$status"
}

trap _term TERM
trap _term INT

echo "Building socat image"
DOCKER_BUILDKIT=1 docker build -t "socat:local" $curpath/socat

echo "Loading into kind"

<<<<<<< HEAD
if [ -z "$BUILD_TAG" ]
then
  cluster_name=agent-dev-cluster
else
  cluster_name=$(echo $BUILD_TAG | tr '[:upper:]' '[:lower:]' | tail -c 32 | sed 's/^-*//g')
fi

=======
>>>>>>> master
kind load docker-image "logdna-agent-v2:local" --name $cluster_name
kind load docker-image "socat:local" --name $cluster_name

echo "Creating k8s resources"
KUBECONFIG=$curpath/.kind_config_host kubectl apply -f $curpath/kind/test-resources.yaml 
KUBECONFIG=$curpath/.kind_config_host kubectl apply -f $curpath/kind/metrics-server.yaml 

# Run the integration test binary in docker on the same network as the kubernetes cluster
if [ "$HOST_MACHINE" = "Mac" ]; then
	# shellcheck disable=SC2086
	child=$(docker run --network $kind_network -d -w "$1" $extra_args -v "$2" -v $curpath/.kind_config:$1/.kind_config -e KUBECONFIG=$1/.kind_config $5 "$4" /bin/sh -c "$6")
elif [ "$HOST_MACHINE" = "Linux" ]; then
	# shellcheck disable=SC2086
	child=$(docker run --network $kind_network -d -u "$(id -u)":"$(id -g)" -w "$1" $extra_args -v "$2" -v $curpath/.kind_config:$1/.kind_config -e KUBECONFIG=$1/.kind_config $5 "$4" /bin/sh -c "$6")
fi


# Allow tailing the logs to fail
set +e
# Tail the container til it's done
docker logs -f "$child"
set -e

# Get the exit code of completed container
echo "Getting results..."
status=$(docker inspect "$child" --format='{{.State.ExitCode}}')

# Clean up the container
docker rm "$child" > /dev/null

$curpath/kind_stop.sh

echo "Exit status: $status"

exit "${status:-1}"
