# LogDNA Agent

[![Rustc Version 1.42+]][rustc] [Join us on LogDNA's Public Slack]

The LogDNA agent is a fast, resource-efficient log collection client that forwards logs to [LogDNA]. This version of the agent is written in [Rust] to ensure maximum performance, and when coupled with LogDNA's web application, provides a powerful log management tool for distributed systems, including [Kubernetes] clusters.

[Rustc Version 1.42+]: https://img.shields.io/badge/rustc-1.42+-lightgray.svg
[rustc]: https://blog.rust-lang.org/2020/03/12/Rust-1.42.html
[Join us on LogDNA's Public Slack]: http://chat.logdna.com/
[LogDNA]: https://logdna.com
[Rust]: https://www.rust-lang.org/
[Kubernetes]: https://kubernetes.io/

## Table of Contents

* [Managing Deployments](#managing-deployments)
  * [Installing on Kubernetes](#installing-on-kubernetes)
    * [Using manifest files](#using-manifest-files)
    * [Using Helm](#using-helm)
  * [Installing on OpenShift](#installing-on-openshift)
  * [Running as Non-Root](#running-as-non-root)
  * [Additional Installation Options](#additional-installation-options)
* [Building](#building-the-logdna-agent)
  * [Building Docker image](#building-docker-image)
* [Configuration](#configuration)
  * [Options](#options)
  * [Configuring the Environment](#configuring-the-environment)
  * [Configuring Lookback](#configuring-lookback)
  * [Configuring Journald](#configuring-journald)
  * [Configuring Kubernetes Events](#configuring-events)

## Managing Deployments

The agent is supported for Kubernetes 1.9+ and Red Hat OpenShift 4.5+ environments.

### Installing on Kubernetes

You can use the manifest YAML files in this repository or use Helm package manager to deploy the agent in your
Kubernetes cluster.

#### Using manifest files

__NOTE__: The Kubernetes manifest YAML files in this repository (and referenced in this
documentation) describe the version of the LogDNA Agent in the current commit
tree and no other version of the LogDNA agent. If you apply the Kubernetes manifest
found in the current tree then your cluster _will_ be running the version described
by the current commit which may not be ready for general use.

Follow the full instructions in the documentation to [deploy on Kubernetes using resource
files](KUBERNETES.md).

#### Using Helm

Visit the documentation to [install the agent on Kubernetes using Helm](HELM.md).

### Installing on OpenShift

Follow the instructions for [deploying the agent on Red Hat®️ OpenShift®️](OPENSHIFT.md).

### Running as Non-Root

By default the agent will run as root. Below are environment-specific instructions for running the agent as a non-root user.

* [Running as Non-Root on Kubernetes](KUBERNETES.md#run-as-non-root)
* [Running as Non-Root on OpenShift](OPENSHIFT.md#run-as-non-root)

If you configure the agent to run as non-root, review the [documentation about enabling "statefulness" for the agent](KUBERNETES.md#enabling-persistent-agent-state).

### Additional Installation Options

More information about managing your deployments is documented for [Kubernetes](KUBERNETES.md) or [OpenShift](OPENSHIFT.md). This includes topics such as

* Version specific upgrade paths
* Collecting system logs through Journald

## Building the LogDNA Agent

Obtain the source code to build the agent from our [GitHub repository](https://github.com/logdna/logdna-agent-v2). You can either download the source files as a .zip file, or clone the repository.

* Use the following commands to clone and then cd into this repository:

```
git clone https://github.com/logdna/logdna-agent-v2.git
cd logdna-agent-v2
```

Next, select how you want to build the agent: as a docker image or as a Linux binary.

### Building Docker image

To build a Docker image of the agent, ensure the Docker command is installed properly, verify that the Docker engine is running, and then run the following command:

```
make build-image
```

The resulting image can be found by listing the images:

```console
foo@bar:~$ docker images
REPOSITORY              TAG                 IMAGE ID            CREATED             SIZE
logdna-agent-v2         dcd54a0             e471b3d8a409        22 seconds ago      135MB
```

You can also obtain the image and review our tagging scheme on [DockerHub](https://hub.docker.com/r/logdna/logdna-agent).

### Building Agent Binary on Linux

The agent requires `v1.42+` of rustc [cargo-make](https://github.com/sagiegurari/cargo-make) to build. If the proper versions of rustc and cargo are installed; then simply run the following command to build the agent:

```
cargo build --release
```

The compiled binary will be built to `./target/release/logdna-agent`.

## Configuration

### Options

The agent accepts configuration from two sources, environment variables and a configuration YAML file. The default configuration yaml file is located at `/etc/logdna/config.yaml`. The following options are available:

| Variable Name(s) | Description | Default |
|-|-|-|
|`LOGDNA_INGESTION_KEY`<br>**Deprecated**: `LOGDNA_AGENT_KEY`|**Required**: The ingestion key associated with your LogDNA account||
|`LOGDNA_CONFIG_FILE`<br>**Deprecated**: `DEFAULT_CONF_FILE`|Path to the configuration yaml|`/etc/logdna/config.yaml`|
|`LOGDNA_HOST`<br>**Deprecated**: `LDLOGHOST`|The host to forward logs to|`logs.logdna.com`|
|`LOGDNA_ENDPOINT`<br>**Deprecated**: `LDLOGPATH`|The endpoint to forward logs to|`/logs/agent`|
|`LOGDNA_USE_SSL`<br>**Deprecated**: `LDLOGSSL`|Whether to use a SSL for sending logs|`true`|
|`LOGDNA_USE_COMPRESSION`<br>**Deprecated**: `COMPRESS`|Whether to compress logs before sending|`true`|
|`LOGDNA_GZIP_LEVEL`<br>**Deprecated**: `GZIP_COMPRESS_LEVEL`|If compression is enabled, this is the gzip compression level to use|`2`|
|`LOGDNA_HOSTNAME`|The hostname metadata to attach to lines forwarded from this agent||
|`LOGDNA_IP`|The IP metadata to attach to lines forwarded from this agent||
|`LOGDNA_TAGS`|Comma separated list of tags metadata to attach to lines forwarded from this agent||
|`LOGDNA_MAC`|The MAC metadata to attach to lines forwarded from this agent||
|`LOGDNA_LOG_DIRS`<br>**Deprecated**: `LOG_DIRS`|Comma separated list of folders to recursively monitor for log events|`/var/log/`|
|`LOGDNA_EXCLUSION_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE`|Comma separated list of glob patterns to exclude files from monitoring <sup>1</sup>|`/var/log/wtmp,/var/log/btmp,/var/log/utmp,/var/log/wtmpx,/var/log/btmpx,/var/log/utmpx,/var/log/asl/**,/var/log/sa/**,/var/log/sar*,/var/log/tallylog,/var/log/fluentd-buffers/**/*,/var/log/pods/**/*`|
|`LOGDNA_EXCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE_REGEX`|Comma separated list of regex patterns to exclude files from monitoring||
|`LOGDNA_INCLUSION_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE`|Comma separated list of glob patterns to includes files for monitoring <sup>1</sup>|`*.log,!(*.*)`|
|`LOGDNA_INCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE_REGEX`|Comma separated list of regex patterns to exclude files from monitoring||
|`LOGDNA_JOURNALD_PATHS`|Comma separated list of paths (directories or files) of journald paths to monitor||
|`LOGDNA_LOOKBACK`|The lookback strategy on startup|`smallfiles`|
|`LOGDNA_LOG_K8S_EVENTS`|Whether the agent should capture Kubernetes events|`always`|
|`LOGDNA_DB_PATH`|The directory the agent will store it's state database. Note that the agent must have write access to the directory and be a persistent volume.||

1. We support [this flavor of globber syntax](https://github.com/CJP10/globber).

### Configuring the Environment

To configure the DaemonSet, modify the envs section of the DaemonSet [`spec.template.spec.containers.0.env`]. For example, to change the hostname add the following environment variable to the `env` list:

```yaml
env:
  - name: LOGDNA_HOSTNAME
    value: my-hostname
```

Check out [Kubernetes documentation](https://kubernetes.io/docs/tasks/inject-data-application/define-environment-variable-container/) for more information about injecting environment variables into applications!

### Configuring Lookback

The lookback strategy determines how the agent handles existing files on agent startup. This strategy is determined by the `LOGDNA_LOOKBACK` variable.

By default, the agent provides a "stateful", or persistent, collection of files that can be referenced whenever the agent is restarted, in order to return (or look back) to see any files that were ingested during the time that the agent was not running. The state file location is defined using the `LOGDNA_DB_PATH` environment variable in the YAML file (the default path is /var/lib/logdna/agent_state.db).

The valid values for this option are:
   * When set to **`none`**:
      * lookback is disabled, and LogDNA Agent will reading new lines as those are added to the file, ignoring the lines that were written before the time the Agent restarted.
   * When set to **`smallfiles`** (default):
       * If there is information in the “state file”, use the last recorded state. 
       * If the file is not present in the “state file” and the file is less than 8KiB, start at the beginning. If the file is larger than 8KiB, start at the end. 
   * When set to **`start`**:
    * If there is information in the “state file”, use the last recorded state. 
    * If the file is not present in the “state file”, start at the beginning. 

**Notes:**
* If you configure the agent to run as non-root, review the [documentation](KUBERNETES.md#enabling-persistent-agent-state) about enabling "statefulness" for the agent.
* When upgrading from LogDNA Agent version 3.0 to 3.1, the state file will initially be empty, so the lookback setting will be used for existing files. After that (i.e. on process restart), the state file will be present and will be used.


### Configuring Journald

If the agent pods have access to journald log files or directories, monitoring can be enabled on them with the `LOGDNA_JOURNALD_PATHS`. Common values include `/var/log/journal` and `/run/systemd/journal`. To specify both, use a comma separated list: `/var/log/journal,/run/systemd/journal`.

Take a look at enabling journald monitoring for [Kubernetes](KUBERNETES.md#collecting-node-journald-logs) or [OpenShift](OPENSHIFT.md#collecting-node-journald-logs).

### Configuring Events

A Kubernetes event is exactly what it sounds like: a resource type that is automatically generated when state changes occur in other resources, or when errors or other messages manifest across the system. Monitoring events is useful for debugging your Kubernetes cluster.

By default, the LogDNA agent captures Kubernetes events (and OpenShift events, as well, since OpenShift is built on top of Kubernetes clusters).

To control whether the LogDNA agent collects Kubernetes events, configure the `LOGDNA_LOG_K8s_EVENTS` environment variable using on of these two values:

* `always` - Always capture events
* `never` - Never capture events
__Note:__ The default option is `always`.

> :warning: Due to a ["won't fix" bug in the Kubernetes API](https://github.com/kubernetes/kubernetes/issues/41743), the LogDNA agent collects events from the entire cluster, including multiple nodes. To prevent duplicate logs when running multiple pods, the LogDNA agent pods defer responsibilty of capturing events to the oldest pod in the cluster. If that pod is down, the next oldest LogDNA agent pod will take over responsibility and continue from where the previous pod left off.
