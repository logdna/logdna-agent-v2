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
kubectl apply -f k8s/agent-resources.yaml
```
3. Monitor the pods for startup success:
```console
foo@bar:~$ kubectl get pods -n logdna-agent --watch
NAME                 READY   STATUS    RESTARTS   AGE
logdna-agent-hcvhn   1/1     Running   0          10s
```

> :warning: By default the agent will run as root. To run the agent as a non-root user, refer to the section [Run as Non-Root](#run-as-non-root) below.

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

> :warning: Exporting Kubernetes objects with "kubectl get \<resource\> -o yaml" includes extra information about the object's state. This data does not need to be copied over to the new YAML file.

#### Upgrading from Configuration v2.1.x

* **Example Configuration YAML Files:**
  * [v2.1.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-v2-beta.yaml)
* **Differences:** The configuration contains the same namespace and Kubernetes objects. The only differences are some changes to the DaemonSet.
* **Upgrade Steps:**
  1. If you have changes you want to persist to the new DaemonSet, backup the old DaemonSet.
     1. Run `kubectl get daemonset -o yaml -n logdna-agent logdna-agent > old-logdna-agent-daemon-set.yaml`.
     2. Copy any desired changes from `old-logdna-agent-daemon-set.yaml` to the DaemonSet object in `k8s/agent-resources.yaml`.
  2. Apply the latest configuration YAML file; run `kubectl apply -f k8s/agent-resources.yaml`.

> :warning: Exporting Kubernetes objects with "kubectl get \<resource\> -o yaml" includes extra information about the object's state. This data does not need to be copied over to the new YAML file.

### Upgrading the Image

The image contains the actual agent code that is run on the Pods created by the DaemonSet. New versions of the agent always strive for backwards compatibility with old configuration versions. Any breaking changes will be outlined in the [release page](https://github.com/logdna/logdna-agent-v2/releases). We always recommend upgrading to the latest configuration to guarantee access to new features.

The upgrade path for the image depends on which image tag you are using in your DaemonSet.

If your DaemonSet is configured with `logdna/logdna-agent:stable`, the default configuration setting, then restarting your Pods will trigger them to pull down the latest stable version of the LogDNA agent image.

```console
kubectl rollout restart daemonset -n logdna-agent logdna-agent
```

Otherwise, if your DaemonSet is configured with a different tag (e.g. `logdna/logdna-agent:2.1.7`), you'll need to update the image and tag, which will trigger a rollover of all the pods.

```console
kubectl patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"logdna/logdna-agent:2.2.0"}]'
```

* `latest` - Update with each new revision including public betas.
* `stable` - Updates with each major, minor, and patch version updates.
* `2` - Updates with each minor and patch version updates under `2.x.x`.
* `2.2` - Updates with each patch version update under `2.2.x`.
* `2.2.0` - Targets a specific version of the agent.

**Note:** This list isn't exhaustive; for a full list check out the [logdna-agent dockerhub page](https://hub.docker.com/r/logdna/logdna-agent)

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

This is accomplished through Linux capabilities and turning the agent binary into a "capability-dumb binary." The binary is given `CAP_DAC_READ_SEARCH` to read all files on the file system. The image already comes with this change and the necessary user and group. The only required step is configuring the agent DaemonSet to run as the user and group `5000:5000`.

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

### Enabling persistent agent state

To avoid skipping or duplicating logs when the agent pod restarts or is replaced the agent needs to store it's progress on the host node's filesystem. This is achieved using a hostPath volume. The host directory must be writable by the user or group specified in the securityContext.

To achieve this the host directory must either already exist with the correct
permissions or else Kubernetes will create the directory without write permissions for the agent user.

In this case the permissions must be set before the agent starts. When running as non-root the agent pod does not have permissions to do this, so an initcontainer may be used.

Below is an example manifest section which can be added to the pod specification alongside the containers array. It assumes the agent user/group are both 5000 and the volume is mounted at /var/lib/logdna

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
