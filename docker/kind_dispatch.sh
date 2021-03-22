#!/usr/bin/env sh

curpath=$(dirname "$0")
image="logdna-agent-kind:building"
create_cluster=${2:-true}

echo "Starting k8s kind build"

# shellcheck source=/dev/null
. "$curpath/lib.sh"

kind --version || exit 1

if [ "$create_cluster" = "true" ]
then
  kind create cluster --name agent-dev-cluster --config=docker/kind/kind-config.yaml
else
  echo "Reusing existing cluster"
  kubectl config use-context kind-agent-dev-cluster || exit 1
fi

echo "Building image"
docker build -t "$image" -f "$curpath/kind/Dockerfile" "$curpath/.."

echo "Loading into kind"
kind load docker-image logdna-agent-kind:building --name agent-dev-cluster

echo "Creating k8s resources"
kubectl apply -f docker/kind/test-resources.yaml
kubectl create namespace agent-dev
kubectl apply -f docker/kind/agent-dev-build.yaml --namespace agent-dev

kubectl wait --for condition=Ready --timeout=30s -n agent-dev pod/agent-dev-build
sleep 2
kubectl logs -n agent-dev agent-dev-build --follow

echo "Getting results..."
sleep 2
status=$(kubectl get pod -n agent-dev agent-dev-build --output="jsonpath={.status.containerStatuses[].state.terminated.exitCode}")

echo "Deleting resources"
if [ "$create_cluster" = "true" ]
then
  kind delete cluster --name agent-dev-cluster
else
  kubectl delete -f docker/kind/agent-dev-build.yaml --namespace agent-dev
  kubectl delete -f docker/kind/test-resources.yaml
fi

echo "Exit status: $status"

exit "${status:-1}"
