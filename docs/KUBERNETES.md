# LogDNA Agent on Kubernetes

The agent is compatible with Kubernetes<sup>:registered:</sup> versions 1.9 and greater; however, we always recommend running the most recent stable version.

__NOTE__: The Kubernetes manifest YAML files in this repository (and referenced in this
documentation) describe the version of the LogDNA Agent in the current commit
tree and no other version of the LogDNA agent. If you apply the Kubernetes manifest
found in the current tree then your cluster _will_ be running the version described
by the current commit which may not be ready for general use.

You MUST ensure that the current tree is checked out to the branch/tag is the
version you intend to install, if you install a pre-release version of the
agent your logs may not be collected.

To install a specific version you should ensure that you check out the exact tag for that
version before applying any YAML files from the current tree.

For example, to install a specific version, say a particular build of a beta, you would run the following command (with the
specific tag's version number) in the repo's root directory before following the install instructions relevant for your cluster.

```bash
git checkout major.minor.patch-beta.n
```

To view a list of all available release versions, navigate in your terminal to your local working directory (where you cloned the agent source repository) and then run the command `git tag -l`.

__NOTE:__
For a quick installation without cloning the GitHub repo, you can run the following three commands in your terminal to create a logdna-agent namespace with your ingestion key and then deploy the LogDNA Agent DaemonSet to your cluster.
```bash
kubectl apply -f https://assets.logdna.com/clients/logdna-agent/3/agent-namespace.yaml
kubectl create secret generic logdna-agent-key -n logdna-agent --from-literal=logdna-agent-key=<your ingestion key> # this is your unique Ingestion Key
kubectl apply -f https://assets.logdna.com/clients/logdna-agent/3/agent-resources.yaml
```

## Table of Contents

* [Installing](#installing)
  * [Installation Prerequisites](#installation-prerequisites)
  * [Installation Steps](#installation-steps)
* [Upgrading](#upgrading)
  * [Upgrading the Configuration](#upgrading-the-configuration)
    * [Upgrading from Configuration v1.x.x or v2.0.x](#upgrading-from-configuration-v1xx-or-v20x)
    * [Upgrading from Configuration v2.1.x](#upgrading-from-configuration-v21x)
  * [Upgrading the Image](#upgrading-the-image)
* [Uninstalling](#uninstalling)
* [Run as Non-Root](#run-as-non-root)
* [Collecting Node Journald Logs](#collecting-node-journald-logs)
  * [Enabling Journald on the Node](#enabling-journald-on-the-node)
  * [Enabling Journald Monitoring on the Agent](#enabling-journald-monitoring-on-the-agent)
* [Configuration for Kubernetes Metadata Filtering](#configuration-for-kubernetes-metadata-filtering)
* [Enabling K8 Events](#enabling-k8-events)
* [Enabling Metrics Reporter Statistics](#enabling-metrics-reporter-statistics)
* [GKE Autopilot](#gke-autopilot)

## Installing

The agent can be installed in your cluster using a set of YAML files we provide. These files contain the minimum necessary Kubernetes objects and settings to run the agent. Teams should review and modify these YAML files for the specific needs of their clusters.

### Installation Prerequisites

* LogDNA Account - Create an account with LogDNA by following our [quick start guide](https://docs.logdna.com/docs/logdna-quick-start-guide).
* LogDNA Ingestion Key - You can find an ingestion key at the top of [your account's Add a Log Source page](https://app.logdna.com/pages/add-host).
* Kubernetes cluster running at least version 1.9.
* Local clone of this repository.

### Installation Steps

1. Navigate to the root directory of the cloned `logdna-agent-v2` repository.
2. Run the following commands to configure and start the agent:
```console
kubectl apply -f k8s/agent-namespace.yaml
kubectl create secret generic logdna-agent-key -n logdna-agent --from-literal=logdna-agent-key=<YOUR LOGDNA INGESTION KEY>
kubectl apply -f k8s/agent-startup-lease.yaml (optional, see note below)
kubectl apply -f k8s/agent-resources.yaml
```
__Optional:__ The `agent-startup-lease.yaml` file only needs to be applied if you intend to use the K8's Lease option in the agent startup process.

By default, there are 5 leases created. The `agent-startup-lease.yaml` file can be modified to include additional startup leases if needed.
3. Monitor the pods for startup success:
```console
kubectl get pods -n logdna-agent --watch
NAME                 READY   STATUS    RESTARTS   AGE
logdna-agent-hcvhn   1/1     Running   0          10s
```

> :warning:  By default the agent will run as root. To run the agent as a non-root user, refer to the section [Run as Non-Root](#run-as-non-root) below.

__NOTE__ The agent provides a "stateful" or persistent set of files that is available for reference whenever the agent is restarted; this allows for a configurable `lookback` option. For details, refer to our documentation about [Configuring Lookback](README.md/#configuring-lookback). To maintain backward compatibility, our default YAML file for Kubernetes environments uses a configuration value of `smallfiles`. However, if you create your own YAML file, the default `lookback` setting will be `none` unless you configure your yaml to enable statefulness and explicitly set it to `smallfiles` or `start`.

## Upgrading

There are two components, the configuration and the image, that can be upgraded independently of each other. While not strictly required, we always recommend upgrading both components together.

### Upgrading the Configuration

Not every version update of the agent makes a change to our supplied configuration YAML files. If there are changes, they will be outlined in the [release page](https://github.com/logdna/logdna-agent-v2/releases).

Depending on what version of the agent configuration you're using, different steps are required to update it. If you are unsure of what version of the configuration you have, you can always check the `app.kubernetes.io/version` label of the DaemonSet:

```console
foo@bar:~$ kubectl describe daemonset --all-namespaces -l app.kubernetes.io/name=logdna-agent
Name:           logdna-agent
Selector:       app=logdna-agent
Node-Selector:  <none>
Labels:         app.kubernetes.io/instance=logdna-agent
                app.kubernetes.io/name=logdna-agent
                app.kubernetes.io/version=2.2.0
...
```

Older versions of our configurations do not provide these labels. In that case, each upgrade path below provides an example of each configuration to compare to what's running on your cluster.

> :warning:  Exporting Kubernetes objects with "kubectl get \<resource\> -o yaml" includes extra information about the object's state. This data does not need to be copied over to the new YAML file.

#### Upgrading from Configuration v1.x.x or v2.0.x

* **Example Configuration YAML Files:**
  * [v1.x.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-ds.yaml)
  * [v2.0.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-v2.yaml)
* **Differences:** The configuration does not include the new logdna-agent namespace and is lacking a number of new Kubernetes objects.
* **Upgrade Steps:**
  1. If you have changes you want to persist to the new DaemonSet, backup the old DaemonSet.
     1. Run `kubectl get daemonset -o yaml logdna-agent > old-logdna-agent-daemon-set.yaml`.
     2. Copy any desired changes from `old-logdna-agent-daemon-set.yaml` to the DaemonSet object in `k8s/agent-resources.yaml`.
  2. Remove the old DaemonSet in the default namespace; run `kubectl delete daemonset logdna-agent`.
  3. [Install the latest agent](#installation-steps).

#### Upgrading from Configuration v2.1.x

* **Example Configuration YAML Files:**
  * [v2.1.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-v2-beta.yaml)
* **Differences:** The configuration contains the same namespace and Kubernetes objects. The only differences are some changes to the DaemonSet.
* **Upgrade Steps:**
  1. If you have changes you want to persist to the new DaemonSet, backup the old DaemonSet.
     1. Run `kubectl get daemonset -o yaml -n logdna-agent logdna-agent > old-logdna-agent-daemon-set.yaml`.
     2. Copy any desired changes from `old-logdna-agent-daemon-set.yaml` to the DaemonSet object in `k8s/agent-resources.yaml`.
  2. Apply the latest configuration YAML file; run `kubectl apply -f k8s/agent-resources.yaml`.


### Upgrading the Image

The image contains the actual agent code that is run on the Pods created by the DaemonSet. New versions of the agent always strive for backwards compatibility with old configuration versions. Any breaking changes will be outlined in the [change log](https://docs.mezmo.com/changelog). We always recommend upgrading to the latest configuration to guarantee access to new features.

The upgrade path for the image depends on which image tag you are using in your DaemonSet.

If your DaemonSet is configured with `logdna/logdna-agent:3`, or some other major version number, then restarting your Pods will trigger them to pull down the latest minor for this major version (in this example `3`) version of the LogDNA agent image.

```console
kubectl rollout restart daemonset -n logdna-agent logdna-agent
```

Otherwise, if your DaemonSet is configured with a different tag (e.g. `logdna/logdna-agent:3.5.1`), you'll need to update the image and tag, which will trigger a rollover of all the pods.

```console
kubectl patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"logdna/logdna-agent:3.5.1"}]'
```

* `3` - Updates with each minor and patch version updates under `3.x.x`.
* `3.5` - Updates with each patch version update under `3.5.x`.
* `3.5.1` - Targets a specific version of the agent.

__NOTE__ This list isn't exhaustive; for a full list check out the [logdna-agent dockerhub page](https://hub.docker.com/r/logdna/logdna-agent)

## Uninstalling

The default configuration places all of the Kubernetes objects in a unique namespace. To completely remove all traces of the agent you need to simply delete this namespace:

```console
kubectl delete -f k8s/agent-namespace.yaml
```

If you're sharing the namespace with other applications, and thus you need to leave the namespace, you can instead remove all traces by deleting the agent with a label filter. You'll also need to remove the logdna-agent-key secret which doesn't have a label:

```console
kubectl api-resources --verbs=list --namespaced -o name | xargs -n 1 kubectl get --show-kind --ignore-not-found -n <NAMESPACE> -l app.kubernetes.io/name=logdna-agent -o name | xargs kubectl delete -n <NAMESPACE>
kubectl delete secret -n <NAMESPACE> logdna-agent-key
```

## Run as Non-Root

By default the agent is configured to run as root; however, the DaemonSet can be modified to run the agent as a non-root user.

This is accomplished through Linux capabilities and turning the agent binary into a "capability-dumb binary." The binary is given `CAP_DAC_READ_SEARCH` to read all files on the file system. The image already comes with this change and the necessary user and group. The only required step is configuring the agent DaemonSet to run as the user and group `5000:5000`. Note that if your UID (User Identifier) and GID (Group Identifier) values are something other than 5000, use your local values.

Add two new fields, `runAsUser` and `runAsGroup`, to the `securityContext` section found in the `logdna-agent` container in the `logdna-agent` DaemonSet inside of `k8s/agent-resources.yaml` [`spec.template.spec.containers.0.securityContext`]:

```yaml
securityContext:
  runAsUser: 5000
  runAsGroup: 5000
```

Apply the updated configuration to your cluster:

```console
kubectl apply -f k8s/agent-resources.yaml
```

Alternatively, update the DaemonSet configuration directly by using the following patch command:

```console
kubectl patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsUser","value":5000},{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsGroup","value":5000}]'
```

### Enabling file offset tracking across restarts

To avoid possible duplication or skipping of log messages during agent restart or upgrade, the agent stores its current file offsets on the host's filesystem, using a `hostPath` volume.

The host directory must be writable by the user or group specified in the `securityContext`.

To achieve this the host directory must either already exist with the correct
permissions or else Kubernetes will create the directory without write permissions for the agent user.

In this case the permissions must be set before the agent starts. When running as non-root the agent pod does not have permissions to do this, so an initcontainer may be used.

Below is an example manifest section which can be added to the pod specification alongside the containers array. It assumes the agent user/group are both `5000` and the volume is mounted at `/var/lib/logdna`:

```console
      initContainers:
        - name: volume-mount-permissions-fix
          image: busybox
          command: ["sh", "-c", "chmod -R 775 /var/lib/logdna && chown -R 5000:5000 /var/lib/logdna"]
          volumeMounts:
          - name: varliblogdna
            mountPath: /var/lib/logdna
```

## Collecting Node Journald Logs

The agent by default only captures logs generated by the containers running on the Kubernetes cluster's container runtime environment. It does not, however, collect system component logs from applications running directly on the node such as the kubelet and container runtime. With some configuration on both the node and the agent, these journald logs can be exposed from the node to the agent.

The agent can access Journald logs from the host node by mounting the logs from `/var/log/journal`. This requires enabling journald log storage in the node as well as configuring the agent to monitor the directory.

### Enabling Journald on the Node

Follow the steps below to ensure journald logs are written to `/var/log/journal`:
1. Login as root on your node.
2. Ensure the `journald.conf`, usually found at `/etc/systemd/`, sets `Storage=persistent`. Look at the [journald.conf documentation](https://www.freedesktop.org/software/systemd/man/journald.conf.html) for more information.
3. Create the directory `/var/log/journal`: `mkdir -p /var/log/journal`.

### Enabling Journald Monitoring on the Agent

To enable Journald monitoring in the agent, add a new environment variable, `LOGDNA_JOURNALD_PATHS` with a value of `/var/log/journal`, to the logdna-agent DaemonSet:
* If you are updating an already deployed agent:
  1. You can patch the existing agent by running.
```console
kubectl patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/env/-","value":{"name":"LOGDNA_JOURNALD_PATHS","value":"/var/log/journal"}}]'
```
* If you are modifying a YAML file:
  1. Add the new environment variable to the envs section of the DaemonSet object in `k8s/agent-resources.yaml` [`spec.template.spec.containers.0.env`].
  2. Apply the new configuration file, run `kubectl apply -f k8s/agent-resources.yaml`.

 ```yaml
 env:
   - name: LOGDNA_JOURNALD_PATHS
     value: /var/log/journal
 ```

### GKE Autopilot

Use `k8s/agent-resources-no-cap.yaml` to deploy Agent in GKE Autopilot or other clusters that do not allow DAC.

__NOTE__ In this "no-cap" deployment Agent does not support the "stateful" mode mentioned in [documentation about enabling "statefulness" for the agent](KUBERNETES.md#enabling-persistent-agent-state).

## Configuration for Kubernetes Metadata Filtering

In addition to being able to write custom regex expressions, the Agent can be configured to filter log lines associated with various Kubernetes resources. Specifically, the agent filters on Kubernetes Pod metadata. For example, include only log lines coming from a given namespace. This can be useful because writing a custom regex expression for filtering out logs coming from a specific resources requires the user to know how Kubernetes formats it's log files. Using the Kubernetes filtering configuration makes this much easier. 

You can configure the Kubernetes Pod metadata filtering via the following environment variables:

* include *only* log lines associated with a specific resource with `LOGDNA_K8S_METADATA_LINE_INCLUSION`
* exclude log lines associated with specific resource with `LOGDNA_K8S_METADATA_LINE_EXCLUSION`

Currently, we support filtering on four different fields: namespace, pod, label and annotation. The following shows the nomenclature for each of the supported fields:

```
  namespace:<namespace-name>
  name:<pod-name>
  label.<key>:<value>
  annotation.<key>:<value>
``` 

The following is a sample configuration:

```
"name": "LOGDNA_K8S_METADATA_LINE_INCLUSION"
"value": "namespace:default"

"name": "LOGDNA_K8S_METADATA_LINE_EXCLUSION"
"value": "label.app.kubernetes.io/name:sample-app, annotation.user:sample-user"
```
 
 In the above configuration, the agent will return all log lines coming from the "default" Kubernetes namespace, but filter out anything with a label key/value of `app.kubernetes.io/name:sample-app` or an annotation with a key/value of `user:sample-user`. If both inclusion and exclusion filters are present in the configuration, the Agent will apply the inclusion rule first, followed by the exclusion rule. Also note that in above example that `label.<key><value>` is equal to `label.<app.kubernetes.io/name>:<sample-app>`, with the `app.kubernetes.io/name` being a common nomenclature for Kubernetes configuration. 

 The Kubernetes filtering uses information from the K8s API. As a result, in order for filtering to work you need to have the `LOGDNA_USE_K8S_LOG_ENRICHMENT` set to `always`; which is the default.   

**Note:**

* As with regex filtering, the exclusion rules are applied after the inclusion rules. Therefore, in order for a lint to be ingested, it needs to match all inclusion rules (if any) and NOT match any exclusion rule.
* For the key/value files, only integers [0-9], lower case letters [a-z] and the characters `.`, `/`, `-` are supported.
* We do not support nested label or annotation structures; only simple key/value pairs.




### Enabling K8 Events

1. Apply Lease for events (https://github.com/logdna/logdna-agent-v2/blob/master/k8s/event-leader-lease.yaml)
2. Enable (set to always): LOGDNA_LOG_K8S_EVENTS

A Kubernetes event is exactly what it sounds like: a resource type that is automatically generated when state changes occur in other resources, or when errors or other messages manifest across the system. Monitoring events is useful for debugging your Kubernetes cluster.

Only one pod in a cluster will report on k8 events for the entire cluster - the leader election process leverages leases. please see the event-leader-leases.yaml file in the k8s folder for the lease specifications, permissions.

By default, the LogDNA agent does not capture Kubernetes events (and OpenShift events, as well, since OpenShift is built on top of Kubernetes clusters).

To control whether the LogDNA agent collects Kubernetes events, configure the `LOGDNA_LOG_K8S_EVENTS` environment variable using one of these two values:

* `always` - Always capture events
* `never` - Never capture events
__Note:__ The default option is `never`.

> :warning: Due to a ["won't fix" bug in the Kubernetes API](https://github.com/kubernetes/kubernetes/issues/41743), the LogDNA agent collects events from the entire cluster, including multiple nodes. To prevent duplicate logs when running multiple pods, the LogDNA agent pods defer responsibilty of capturing events to the oldest pod in the cluster. If that pod is down, the next oldest LogDNA agent pod will take over responsibility and continue from where the previous pod left off.

### Enabling Metrics Reporter Statistics

1. Start metrics server (https://github.com/kubernetes-sigs/metrics-server#installation)
2. Apply Lease for reporter (https://github.com/logdna/logdna-agent-v2/blob/master/k8s/reporter-leader-lease.yaml)
3. Enable (set to always): LOGDNA_LOG_METRIC_SERVER_STATS

With this enabled the agent will pull from the kubernetes metrics-server, this allows the agent to report on CPU/Memory usage statistics for pods and nodes in the cluster. These statistics will be viewable on the web application for individual log lines showing usage for the pod and node associated with that log line.

Reach out to Mezmo to get this feature enabled on the web application in addition to this configuration.

Only one pod in a cluster will report metrics statistics for the entire cluster - the leader election process leverages leases. please see the reporter-leader-leases.yaml file in the k8s folder for the lease specifications, permissions.

To control whether the LogDNA agent reports usage statistics use the `LOGDNA_LOG_METRIC_SERVER_STATS` environment variable with one of these two values:

* `always` - Always report usage
* `never` - Never report usage
__Note:__ The default option is `never`.

#### Cluster Tag to distinguish between multiple clusters

When you are sending log lines to Mezmo from multiple similarly named clusters you will need to add a value to the `MZ_TAGS` environment variable.  Please add a tag that is prefixed `k8sclusterkey_` (for example `k8sclusterkey_prod-clstr` where "prod-clstr" in "k8sclusterkey_prod-clstr" is a unique cluster name).  This will allow enrichment to distinguish between the clusters when the nodes are similarly named.

This will also add a new line identifier called **Cluster** to your log lines which can be searched against.