#!/usr/bin/env sh

curpath=$(dirname "$0")
create_cluster=${1:-true}
cluster_name=${2:-agent-dev-cluster}

KIND_IMAGE=kindest/node:v1.23.4@sha256:0e34f0d0fd448aa2f2819cfd74e99fe5793a6e4938b328f657c8e3f81ee0dfb9

>&2 echo "Starting k8s kind build"

# shellcheck source=/dev/null
>&2 . "$curpath/lib.sh"

>&2 kind --version || exit 1

export KIND_EXPERIMENTAL_DOCKER_NETWORK=$cluster_name
if [ "$create_cluster" = "true" ]
then
  >&2 kind create cluster --name $cluster_name \
      --image=$KIND_IMAGE \
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

api_server_node_port=$(echo $api_server_node_addr | cut -f2 -d":")

sed "s#server: https://.*#server: https://${cluster_name}-control-plane:$api_server_node_port#" $curpath/.kind_config_host > $curpath/.kind_config
chmod 744 $curpath/.kind_config

echo $KIND_EXPERIMENTAL_DOCKER_NETWORK
