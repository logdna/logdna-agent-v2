#!/usr/bin/env sh

curpath=$(dirname "$0")
image="logdna-agent-kind:building"
create_cluster=${2:-true}

echo "Deleting resources"
if [ "$create_cluster" = "true" ]
then
  kind delete cluster --name agent-dev-cluster
else
  kubectl delete -f docker/kind/agent-dev-build.yaml --namespace agent-dev
  kubectl delete -f docker/kind/test-resources.yaml
fi
