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

Run the following commands to install the LogDNA Agent with the [release name][helm-concepts] `my-release` in your
cluster:

```bash
$ helm repo add logdna https://assets.logdna.com/charts
$ helm install --set logdna.key=$LOGDNA_INGESTION_KEY my-release logdna/agent
```

You should see logs in https://app.logdna.com in a few seconds.

### Using a namespace

If you want to scope your release to a namespace, you can use Helm's `-n` flag when installing:

```bash
helm install --set logdna.key=$LOGDNA_INGESTION_KEY -n my-namespace --create-namespace my-release logdna/agent
```

### Tags support:

Optionally, you can configure the LogDNA Agent to associate tags to all log records that it collects so that you can
identify the data more quickly in the LogDNA web UI.

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
`podSecurityContext` | The `securityContext` for the agent pods | `add: DAC_READ_SEARCH, drop: all`
`nodeSelector` | The `nodeSelector` for the agent pods | None
`podAnnotations` | Additional `annotations` for the agent pods | None
`secretAnnotations` | Additional `annotations` for the agent secret(s) | None

Specify each parameter using the `--set key=value[,key=value]` argument to `helm install`. For example:

```bash
$ helm install --set logdna.key=LOGDNA_INGESTION_KEY,logdna.tags=production my-release logdna/agent
```

In special cases like `extraEnv`, you will need to set each individual item in the array when using `set`:

```bash
$ helm install --set logdna.key=LOGDNA_INGESTION_KEY,logdna.tags=production,extraEnv[0].name=LOGDNA_LOG_K8S_EVENTS,extraEnv[0].value=true my-release logdna/agent
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

## Upgrading from the archived charts repository

Charts for the LogDNA Agent were previously available on the [archived "stable" charts repository][helm-stable].
If you deployed the LogDNA Agent using those helm charts, you can use `helm upgrade` to update your helm installation.

First, locate the release name used to deploy:

```bash
helm list -A
```

Next, use the namespace and release name to run the upgrade command:

```bash
helm upgrade --set logdna.key=$LOGDNA_INGESTION_KEY --set nameOverride=logdna-agent -n my-namespace my-release logdna/agent
```

In case the upgrade fails due to invalid `spec.selector`, make sure to use the same label:

```bash
kubectl describe daemonsets -n my-namespace | grep Selector
```

Use the `app.kubernetes.io/name` value to set `nameOverride` value:

```bash
helm upgrade --set logdna.key=$LOGDNA_INGESTION_KEY --set nameOverride=logdna-agent -n my-namespace my-release logdna/agent
```

[helm]: https://helm.sh/
[helm-install]: https://helm.sh/docs/intro/install/
[helm-concepts]: https://helm.sh/docs/intro/using_helm/#three-big-concepts
[helm-stable]: https://github.com/helm/charts#%EF%B8%8F-deprecation-and-archive-notice
