# Deploying the LogDNA Agent on Kubernetes using Helm

[Helm][helm] is a package manager for Kubernetes that you can use to deploy the LogDNA Agent in your Kubernetes
cluster.

## Prerequisites

- [Helm 3+][helm-install]
- Kubernetes 1.13+

## Obtaining the ingestion key

Follow directions from https://app.logdna.com/pages/add-source to obtain your LogDNA ingestion key.

## Installing the Chart

A [Helm Chart][helm-concepts], defined in a package containing a set of YAML files, acts a single point of authority
and provides repeatable build and deploy tasks to define, install, and upgrade Kubernetes resources.

Run the following commands to install the agent with the [release name][helm-concepts] `my-release` in your cluster:

```bash
$ helm repo add logdna https://assets.logdna.com/charts
$ helm install --set logdna.key=$LOGDNA_INGESTION_KEY my-release logdna/agent
```

You should see logs in https://app.logdna.com in a few seconds.

### Tags support:

Optionally, you can configure the LogDNA Agent to associate tags to all log records that it collects so that you can
identify the agent's data quicker in the LogDNA web UI.

```bash
$ helm install --set logdna.key=$LOGDNA_INGESTION_KEY,logdna.tags=production my-release logdna/agent
```

## Configuration

The following table lists the configurable parameters of the LogDNA Agent chart and their default values. Note
that only the ingestion key is required.

Parameter | Description | Default
--- | --- | ---
`daemonset.tolerations` | List of node taints to tolerate | `[]`
`daemonset.updateStrategy` | Optionally set an update strategy on the daemonset. | None
`image.pullPolicy` | Image pull policy | `IfNotPresent`
`logdna.key` | LogDNA Ingestion Key (Required) | None
`logdna.tags` | Optional tags such as `production` | None
`priorityClassName` | (Optional) Set a PriorityClass on the Daemonset | `""`
`resources.limits.memory` | Memory resource limits | 75Mi
`updateOnSecretChange` | Optionally set annotation on daemonset to cause deploy when secret changes | None
`extraEnv` | Additional environment variables | `{}`
`extraVolumeMounts` | Additional Volume mounts | `[]`
`extraVolumes` | Additional Volumes | `[]`
`serviceAccount.create` | Whether to create a service account for this release | `true`
`serviceAccount.name` | The name of the service account. Defaults to `logdna-agent` unless `serviceAccount.create=false` in which case it defaults to `default` | None

Specify each parameter using the `--set key=value[,key=value]` argument to `helm install`. For example:

```bash
$ helm install --set logdna.key=LOGDNA_INGESTION_KEY,logdna.tags=production my-release logdna/agent
```

Alternatively, create a YAML file that specifies the values for the above parameters. Specify the name of the file
with the values during the chart installation. For example, using a file named `values.yaml`:

```bash
$ helm install -f values.yaml my-release logdna/agent
```

## Uninstalling the Chart

To remove the instance of the Helm chart `my-release` in your cluster:

```bash
$ helm uninstall my-release
```

The command removes all the Kubernetes components associated with the chart and deletes the release.

[helm]: https://helm.sh/
[helm-install]: https://helm.sh/docs/intro/install/
[helm-concepts]: https://helm.sh/docs/intro/using_helm/#three-big-concepts