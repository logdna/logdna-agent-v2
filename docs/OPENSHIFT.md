# LogDNA Agent on OpenShift

The agent has been tested on OpenShift 4.4, but should be compatible with any OpenShift cluster packaged with Kubernetes version 1.9 or greater.

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

The agent can be effortless installed in your cluster using a set of yamls we provide. These yamls contain the minimum necessary OpenShift Objects and settings to run the agent. Teams should review and modify these yamls for the specific needs of their clusters.

### Installation Prerequisites

* LogDNA Account - Create an account with LogDNA by following our [quick start guide](https://docs.logdna.com/docs/logdna-quick-start-guide).
* LogDNA Ingestion Key - You can find an ingestion key at the top of [your account's Add a Log Source page](https://app.logdna.com/pages/add-host).
* OpenShift cluster running Kubernetes 1.9 or greater.
* Local clone of this repository.

### Installation Steps

1. Navigate to the root directory of the cloned `logdna-agent` repository.
2. Run the following commands to create and configure a new project, secret, and service account:
```console
oc new-project logdna-agent
oc create serviceaccount logdna-agent
oc create secret generic logdna-agent-key --from-literal=logdna-agent-key=<YOUR LOGDNA INGESTION KEY>
oc adm policy add-scc-to-user privileged system:serviceaccount:logdna-agent:logdna-agent
```
3. Create the remaining resources:
```console
oc apply -f k8s/agent-resources-openshift.yaml
```
4. Monitor the pods for startup success:
```console
foo@bar:~$ oc get pods --watch
NAME                 READY   STATUS    RESTARTS   AGE
logdna-agent-jb2rg   1/1     Running   0          7s
```

> :warning: By default the agent will run as root. To run the agent as a non-root user, check out the [run as non-root section](#run-as-non-root).


## Upgrading

There are two components that can be upgraded independent of each other for each new version of the agent. While not strictly required, we always recommend upgrading both components together.

### Upgrading the Configuration

Not every version update of the agent makes a change to our supplied configuration yamls. These changes will be outlined in our release page to help you determine if you need to update your configuration.

Due to how the agent has evolved over time, certain versions of the agent configuration yamls require different paths to be updated successfully.

If you are unsure of what version of the configuration you have, you can always check the `app.kubernetes.io/version` label of the DaemonSet:

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

Older versions of our configurations do not provide these labels. In that case, each upgrade path belows provides an example of each configuration to be compared to whats running on your cluster.

#### Upgrading from Configuration v1.x.x

* **Example Configuration Yamls:**
  * [v1.x.x](https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-ds-os.yaml)
* **Differences:** This configuration is lacking a number of new OpenShift Objects. It also uses a removed environment variable for controlling journald monitoring, `USEJOURNALD`.
* **Upgrade Steps:**
  1. If you have changes you want to persist to the new DaemonSet, backup the old DaemonSet.
     1. Run `oc get daemonset -o yaml logdna-agent > old-logdna-agent-daemon-set.yaml`.
     2. Copy any desired changes from `old-logdna-agent-daemon-set.yaml` to the DaemonSet Object in `k8s/agent-resources-openshift.yaml`
  2. If you want to continue using journald, follow the steps for [enabling journald monitoring on the agent](#enabling-journald-monitoring-on-the-agent).
  3. Overwrite the DaemonSet as well as create the new OpenShift Objects.
     1. Run `oc apply -f k8s/agent-resources-openshift.yaml`

> :warning: Exporting OpenShift Objects with "oc get \<resource\> -o yaml" includes extra information about the Object's state. This data does not need to be copied over to the new yaml.

### Upgrading the Image

The image contains the actual agent code that is run on the Pods created by the DaemonSet. New versions of the agent always strive to be backwards compatibility with old configuration versions. Any breaking changes will be outlined on our release page. We always recommend upgrading to the latest configuration to get the best feature support for the agent.

The upgrade path for the image depends on which image tag you are using in your DaemonSet.

If your DaemonSet is configured with `logdna/logdna-agent:stable`, our default configuration setting, then you just need to delete the pods to trigger them to recreate and pull down the latest stable version of the logdna-agent image.

```console
oc delete pod -l app.kubernetes.io/name=logdna-agent
```

Otherwise, if your DaemonSet is configured with a different tag e.g. `logdna/logdna-agent:2.1.7`, you'll need to update the image and tag which will trigger a roll over of all the pods.

```console
oc patch daemonset logdna-agent --type json -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"logdna/logdna-agent:2.2.0"}]'
```

The specific tag you should use depends on your requirements, we offer a list of tags for varying compatibility:
1. `stable` - Updates with each major, minor, and patch version updates
2. `2` - Updates with each minor and patch version updates under `2.x.x`
3. `2.2` - Updates with each patch version update under `2.2.x`
4. `2.2.0` - Targets a specific version of the agent
5. **Note:** This list isn't exhaustive; for a full list check out the [logdna-agent dockerhub page](https://hub.docker.com/r/logdna/logdna-agent)

## Uninstalling

The default configuration places all of the OpenShift Objects in a unique project. To completely remove all traces of the agent you need to simply delete the `logdna-agent` project within the Web UI.

__Note__: OpenShift has no way to delete projects with the `oc` CLI. View OpenShift's documentation for [managing projects](https://docs.openshift.com/container-platform/4.6/applications/projects/working-with-projects.html).

If you're sharing the project with other applications, you can also remove all traces of the agent by deleting with a label filter. You'll also need to remove the `logdna-agent-key` secret and `logdna-agent` service account, both of which do not have a label:

```console
oc api-resources --verbs=list --namespaced -o name | xargs -n 1 oc get --show-kind --ignore-not-found -l app.kubernetes.io/name=logdna-agent -o name | xargs oc delete
oc delete secret logdna-agent-key
oc delete serviceaccount logdna-agent
```

## Run as Non-Root

By default the agent is configured to run as root. If this behavior is not desired, the DaemonSet can be modified to run the agent as a non-root user.

This is accomplished through Linux Capabilities that makes the agent a "Capability-dumb binary." Specifically, the agent is only allowed global read access with `CAP_DAC_READ_SEARCH`. This capability is already baked into the image and configuration. The only required step is configuring the agent DaemonSet to run as the user and group `5000:5000`.

To update the local configuration file, add two new fields, `runAsUser` and `runAsGroup`, to the `securityContext` section found in the `logdna-agent` container in the `logdna-agent` DaemonSet inside of `k8s/agent-resources-openshift.yaml` [`spec.template.spec.containers.0.securityContext`]:

```yaml
securityContext:
  runAsUser: 5000
  runAsGroup: 5000
```

Apply the new updated configuration to your cluster:

```console
oc apply -f k8s/agent-resources-openshift.yaml
```

Alternatively, to update the DaemonSet configuration directly in your cluster use the following patch command:

```console
oc patch daemonset logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsUser","value":5000},{"op":"add","path":"/spec/template/spec/containers/0/securityContext/runAsGroup","value":5000}]'
```

## Collecting Node Journald Logs

The agent by default only captures logs generated by the containers running on the OpenShift clusters container runtime environment. It does not; however, collect system component logs from applications running directly on the node such as the kubelet and container runtime. With some configuration on both the node and the agent, these journald logs can be exposed from the node to the agent.

The agent can access Journald logs from the host node by mounting the logs from `/var/log/journald`. This requires enabling journald log storage in the node as well as configuring the agent to monitor the directory.

### Enabling Journald on the Node

Follow OpenShifts documentation for [enabling Journald on your nodes](https://docs.openshift.com/container-platform/4.6/logging/config/cluster-logging-systemd.html).

### Enabling Journald Monitoring on the Agent

To enable Journald monitoring in the agent, add a new environment variable, `LOGDNA_JOURNALD_PATHS` with a value of `/var/log/journald`, to the logdna-agent DaemonSet:
* If you are updating an already deployed agent:
  1. You can patch the existing agent by running
```console
oc patch daemonset -n logdna-agent logdna-agent --type json -p '[{"op":"add","path":"/spec/template/spec/containers/0/env/-","value":{"name":"LOGDNA_JOURNALD_PATHS","value":"/var/log/journald/-"}}]'
```
* If you are modifying a yaml:
  1. Add the new environment variable to the envs section of the DaemonSet Object in `k8s/agent-resources-openshift.yaml` [`spec.template.spec.containers.0.env`]
  2. Apply the new configuration file, run `oc apply -f k8s/agent-resources-openshift.yaml`

 ```yaml
 env:
   - name: LOGDNA_JOURNALD_PATHS
     value: /var/log/journald
 ```
