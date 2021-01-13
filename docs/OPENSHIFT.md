# LogDNA Agent on OpenShift

The agent is supported for Red Hat<sup>:registered:</sup> OpenShift<sup>:registered:</sup> 4.5 and newer.

## Table of Contents

* [Installing](#installing)
  * [Installation Prerequisites](#installation-prerequisites)
  * [Installation Steps](#installation-steps)
* [Upgrading](#upgrading)
  * [Upgrading the Configuration](#upgrading-the-configuration)
    * [Upgrading from Configuration v1.x.x](#upgrading-from-configuration-v1xx)
  * [Upgrading the Image](#upgrading-the-image)
* [Uninstalling](#uninstalling)
* [Run as Non-Root](#run-as-non-root)
* [Collecting Node Journald Logs](#collecting-node-journald-logs)
  * [Enabling Journald on the Node](#enabling-journald-on-the-node)
  * [Enabling Journald Monitoring on the Agent](#enabling-journald-monitoring-on-the-agent)

## Installing

The agent can be installed in your cluster using a set of YAML files we provide. These files contain the minimum necessary OpenShift objects and settings to run the agent. Teams should review and modify these YAML files for the specific needs of their clusters.

### Installation Prerequisites

* LogDNA Account - Create an account with LogDNA by following our [quick start guide](https://docs.logdna.com/docs/logdna-quick-start-guide).
* LogDNA Ingestion Key - You can find an ingestion key at the top of [your account's Add a Log Source page](https://app.logdna.com/pages/add-host).
* OpenShift cluster running Kubernetes 1.9 or greater.
* Local clone of this repository.

### Installation Steps

1. Navigate to the root directory of the cloned `logdna-agent-v2` repository.
2. Run the following commands to create and configure a new project, secret, and service account:
```console
oc new-project logdna-agent
oc create serviceaccount logdna-agent
oc create secret generic logdna-agent-key --from-literal=logdna-agent-key=<YOUR LOGDNA INGESTION KEY>
oc adm policy add-scc-to-user privileged system:serviceaccount:logdna-agent:logdna-agent
```
3. Create the remaining resources:
```console
oc apply -f https://raw.githubusercontent.com/logdna/logdna-agent-v2/3.0/k8s/agent-resources-openshift.yaml
```
4. Monitor the pods for startup success:
```console
foo@bar:~$ oc get pods --watch
NAME                 READY   STATUS    RESTARTS   AGE
logdna-agent-jb2rg   1/1     Running   0          7s
```

> :warning: By default the agent will run as root. To run the agent as a non-root user, refer to the section [Run as Non-Root](#run-as-non-root) below.

**Note:** To run as non-root, your OpenShift container must still be marked as privileged.

## Upgrading

There are two components, the configuration and the image, that can be upgraded independently of each other. While not strictly required, we always recommend upgrading both components together.

### Upgrading your Configuration

Not every version update of the agent makes a change to our supplied configuration YAML files. If there are changes, they will be outlined in the [release page](https://github.com/logdna/logdna-agent-v2/releases).

Depending on what version of the agent configuration you're using, different steps are required to update it. If you are unsure of what version of the configuration you have, you can always check the `app.kubernetes.io/version` label of the DaemonSet:

```console
foo@bar:~$ oc describe daemonset -l app.kubernetes.io/name=logdna-agent
Name:           logdna-agent
Selector:       app=logdna-agent
Node-Selector:  <none>
Labels:         app.kubernetes.io/instance=logdna-agent
                app.kubernetes.io/name=logdna-agent
                app.kubernetes.io/version=2.2.0
...
```

Older versions of our configurations do not provide these labels. In that case, each upgrade path below provides an example of each configuration to compare to what's running on your cluster.

#### Upgrading from Configuration v1.x.x

* **Example Configuration YAML Files:**
  * [v1.x.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-ds-os.yaml)
* **Differences:** The configuration is lacking a number of new OpenShift objects. It also uses a removed environment variable for controlling journald monitoring, `USEJOURNALD`.
* **Upgrade Steps:**
  1. If you have changes you want to persist to the new DaemonSet, backup the old DaemonSet.
     1. Run `oc get daemonset -o yaml logdna-agent > old-logdna-agent-daemon-set.yaml`.
     2. Copy any desired changes from `old-logdna-agent-daemon-set.yaml` to the DaemonSet object in `k8s/agent-resources-openshift.yaml`.
  2. If you want to continue using journald, follow the steps for [enabling journald monitoring on the agent](#enabling-journald-monitoring-on-the-agent).
  3. Overwrite the DaemonSet as well as create the new OpenShift objects; run `oc apply -f k8s/agent-resources-openshift.yaml`.

> :warning: Exporting OpenShift objects with "oc get \<resource\> -o yaml" includes extra information about the object's state. This data does not need to be copied over to the new YAML file.

### Upgrading your Image

The image contains the actual agent code that is run on the Pods created by the DaemonSet. New versions of the agent always strive for backwards compatibility with old configuration versions. Any breaking changes will be outlined in the [release page](https://github.com/logdna/logdna-agent-v2/releases). We always recommend upgrading to the latest configuration to guarantee access to new features.

The upgrade path for the image depends on which image tag you are using in your DaemonSet.

If your DaemonSet is configured with `logdna/logdna-agent:stable`, the default configuration setting, then restarting your Pods will trigger them to pull down the latest stable version of the LogDNA agent image.

```console
oc rollout restart daemonset logdna-agent
```

Otherwise, if your DaemonSet is configured with a different tag (e.g. `logdna/logdna-agent:2.1.7`), you'll need to update the image and tag, which will trigger a rollover of all the pods.

```console
oc patch daemonset logdna-agent --type json -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"logdna/logdna-agent:2.2.0"}]'
```

The specific tag you should use depends on your requirements, we offer a list of tags for varying compatibility:
* `latest` - Update with each new revision including public betas.
* `stable` - Updates with each major, minor, and patch version updates.
* `2` - Updates with each minor and patch version updates under `2.x.x`.
* `2.2` - Updates with each patch version update under `2.2.x`.
* `2.2.0` - Targets a specific version of the agent.

**Note:** This list isn't exhaustive; for a full list check out the [logdna-agent dockerhub page](https://hub.docker.com/r/logdna/logdna-agent)

## Uninstalling

The default configuration places all of the OpenShift objects in a unique project. To completely remove all traces of the agent you need to simply delete the `logdna-agent` project within the Web UI.

__Note__: OpenShift has no way to delete projects with the `oc` CLI. View OpenShift's documentation for [managing projects](https://docs.openshift.com/container-platform/4.6/applications/projects/working-with-projects.html).

If you're sharing the project with other applications, and thus you need to leave the project, you can instead remove all traces by deleting the agent with a label filter. You'll also need to remove the `logdna-agent-key` secret and `logdna-agent` service account, both of which do not have a label:

```console
oc api-resources --verbs=list --namespaced -o name | xargs -n 1 oc get --show-kind --ignore-not-found -l app.kubernetes.io/name=logdna-agent -o name | xargs oc delete
oc delete secret logdna-agent-key
oc delete serviceaccount logdna-agent
```

## Run as Non-Root

By default the agent is configured to run as root; however, the DaemonSet can be modified to run the agent as a non-root user.

**Note:** To run as non-root the agent container must still be marked as privileged.

This is accomplished through Linux capabilities and turning the agent binary into a "capability-dumb binary." The binary is given `CAP_DAC_READ_SEARCH` to read all files on the file system. The image already comes with this change and the necessary user and group. The only required step is configuring the agent DaemonSet to run as the user and group `5000:5000`.

Add two new fields, `runAsUser` and `runAsGroup`, to the `securityContext` section found in the `logdna-agent` container in the `logdna-agent` DaemonSet inside of `k8s/agent-resources-openshift.yaml` [`spec.template.spec.containers.0.securityContext`]:

```yaml
securityContext:
  runAsUser: 5000
  runAsGroup: 5000
```

Apply the updated configuration to your cluster:

```console
oc apply -f k8s/agent-resources-openshift.yaml
```

Alternatively, update the DaemonSet configuration directly by using the following patch command:

```console
oc patch daemonset logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsUser","value":5000},{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsGroup","value":5000}]'
```

## Collecting Node Journald Logs

The agent by default only captures logs generated by the containers running on the OpenShift cluster's container runtime environment. It does not, however, collect system component logs from applications running directly on the node such as the kubelet and container runtime. With some configuration on both the node and the agent, these journald logs can be exposed from the node to the agent.

The agent can access Journald logs from the host node by mounting the logs from `/var/log/journal`. This requires enabling journald log storage in the node as well as configuring the agent to monitor the directory.

### Enabling Journald on the Node

Follow OpenShift's documentation for [enabling Journald on your nodes](https://docs.openshift.com/container-platform/4.6/logging/config/cluster-logging-systemd.html).

### Enabling Journald Monitoring on the Agent

To enable Journald monitoring in the agent, add a new environment variable, `LOGDNA_JOURNALD_PATHS` with a value of `/var/log/journal`, to the logdna-agent DaemonSet:
* If you are updating an already deployed agent:
  1. You can patch the existing agent by running.
```console
oc patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/env/-","value":{"name":"LOGDNA_JOURNALD_PATHS","value":"/var/log/journal"}}]'
```
* If you are modifying a YAML file:
  1. Add the new environment variable to the envs section of the DaemonSet object in `k8s/agent-resources-openshift.yaml` [`spec.template.spec.containers.0.env`].
  2. Apply the new configuration file, run `oc apply -f k8s/agent-resources-openshift.yaml`.

 ```yaml
 env:
   - name: LOGDNA_JOURNALD_PATHS
     value: /var/log/journal
 ```
