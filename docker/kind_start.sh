#!/usr/bin/env sh

curpath=$(dirname "$0")
create_cluster=${1:-true}

>&2 echo "Starting k8s kind build"

# shellcheck source=/dev/null
>&2 . "$curpath/lib.sh"

>&2 kind --version || exit 1

if [ -z "$BUILD_TAG" ]
then
  cluster_name=agent-dev-cluster
else
  cluster_name=$(echo $BUILD_TAG | tr '[:upper:]' '[:lower:]' | tail -c 32)
fi

export KIND_EXPERIMENTAL_DOCKER_NETWORK=$cluster_name
if [ "$create_cluster" = "true" ]
then
  >&2 kind create cluster --name $cluster_name \
      --config=$curpath/kind/kind-config.yaml \
      --kubeconfig=$curpath/.kind_config_host
else
  >&2 echo "Reusing existing cluster"
  stat $curpath/.kind_config_host > /dev/null || exit 1
fi

api_server_node_addr=$(KUBECONFIG=$curpath/.kind_config_host \
    kubectl cluster-info --context kind-$cluster_name dump | \
    grep kubeadm.kubernetes.io/kube-apiserver.advertise-address.endpoint | \
    awk '{ print length, $2}' | \
    sort -n | \
    cut -d" " -f2- | \
    head -n1 | \
    tr -d ',' | tr -d '"')

sed "s#server: https://.*#server: https://$api_server_node_addr#" $curpath/.kind_config_host > $curpath/.kind_config
chmod 744 $curpath/.kind_config

echo $KIND_EXPERIMENTAL_DOCKER_NETWORK