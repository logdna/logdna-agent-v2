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
  * [Installing](#installing)
  * [Upgrading](#upgrading)
  * [Uninstalling](#uninstalling)
  * [Running as Non-Root](#running-as-non-root)
  * [Additional Installation Options](#additional-installation-options)
* [Building](#building)
  * [Building Docker image](#building-on-docker)
* [Configuration](#configuration)
  * [Options](#options)
  * [Configuring the Environment](#configuring-the-environment)
  * [Configuring Lookback](#configuring-lookback)

## Managing Deployments

The agent is supported for Kubernetes<sup>:registered:</sup> 1.9+ and Red Hat<sup>:registered:</sup> OpenShift<sup>:registered:</sup> 4.5+ environments.

### Installing

__NOTE__: The Kubernetes manifest YAML files in this repository (and referenced in this
documentation) describe the version of the LogDNA Agent in the current commit
tree and no other version of the LogDNA agent. If you apply the kubernetes manifest
found in the current tree then your cluster _will_ be running the version described
by the current commit which may not be ready for general use.

You MUST ensure that the current tree is checked out to the branch/tag is the
version you intend to install, if you install a pre-release version of the
agent your logs may not be collected.

To install a specific version you should ensure you checkout the tag for that
version before applying any YAML files from the current tree.

For example to install a particular 2.2 beta you would run the following command
in the repo's root directory before following the install instructions relevant
for your cluster.

```bash
git checkout 2.2.0-beta.10
```

* [Installing on Kubernetes](KUBERNETES.md#installing)
* [Installing on OpenShift](OPENSHIFT.md#installing)

### Upgrading

* [Upgrading on Kubernetes](KUBERNETES.md#upgrading)
* [Upgrading on OpenShift](OPENSHIFT.md#upgrading)

### Uninstalling

* [Uninstalling on Kubernetes](KUBERNETES.md#uninstalling)
* [Uninstalling on OpenShift](OPENSHIFT.md#uninstalling)

### Running as Non-Root

By default the agent will run as root. Below are environment-specific instructions for running the agent as a non-root user.

* [Running as Non-Root on Kubernetes](KUBERNETES.md#run-as-non-root)
* [Running as Non-Root on OpenShift](OPENSHIFT.md#run-as-non-root)

### Additional Installation Options

More information about managing your deployments is documented for [Kubernetes](KUBERNETES.md) or [OpenShift](OPENSHIFT.md). This includes topics such as

* Version specific upgrade paths
* Collecting system logs through Journald

## Building Agent v2

Clone and cd into this repository:

```
git clone https://github.com/logdna/logdna-agent-v2.git
cd logdna-agent-v2
```

### Building Docker image

To build a Docker<sup>:registered:</sup> image of the agent, ensure the Docker command is installed properly, verify that the Docker engine is running, and then run the following command:

```
make build-image
```

The resulting image can be found by listing the images:

```console
foo@bar:~$ docker images
REPOSITORY              TAG                 IMAGE ID            CREATED             SIZE
logdna-agent-v2         dcd54a0             e471b3d8a409        22 seconds ago      135MB
```

### Building agent binary on Linux

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

The lookback strategy determines how the agent handles existing files on startup. This strategy is determined by `LOGDNA_LOOKBACK`. The set of valid values for this option are:

* `start` - Always start at the beginning of the file
* `smallfiles` - If the file is less than 8KiB, start at the beginning. Otherwise, start at the end
* `none` - Always start at the end of the file
* __Note:__ The default option is `smallfiles`.

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
