#!/usr/bin/env sh

curpath=$(dirname "$0")
create_cluster=${2:-true}

if [ -z "$BUILD_TAG" ]
then
  cluster_name=agent-dev-cluster
else
  cluster_name=$(echo $BUILD_TAG | tr '[:upper:]' '[:lower:]' | tail -c 32)
fi

echo "Deleting resources"
if [ "$create_cluster" = "true" ]
then
  kind delete cluster --name $cluster_name
else
  kubectl delete -f docker/kind/agent-dev-build.yaml --namespace agent-dev
  kubectl delete -f docker/kind/test-resources.yaml
fi
