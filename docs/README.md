# LogDNA Agent

[![Rustc Version 1.42+]][rustc] [Join us on LogDNA's Public Slack]

The LogDNA agent is a blazingly fast, resource efficient log collection client, that forwards logs to [LogDNA]. The 2.0+ version of this agent is written in [Rust] to ensure maximum performance, and when coupled with LogDNA's web application, provides a powerful log management tool for distributed systems, including [Kubernetes] clusters.

![LogDNA Dashboard]

[Rustc Version 1.42+]: https://img.shields.io/badge/rustc-1.42+-lightgray.svg
[rustc]: https://blog.rust-lang.org/2020/03/12/Rust-1.42.html
[Join us on LogDNA's Public Slack]: http://chat.logdna.com/
[LogDNA]: https://logdna.com
[Rust]: https://www.rust-lang.org/
[Kubernetes]: https://kubernetes.io/
[LogDNA Dashboard]: https://files.readme.io/ac5200b-Screen_Shot_2019-07-09_at_7.52.28_AM.png

## Table of Contents

* [Managing Deployments](#managing-deployments)
  * [Installing](#installing)
  * [Upgrading](#upgrading)
  * [Uninstalling](#uninstalling)
  * [More Information](#more-information)
* [Building](#building)
  * [Building on Linux](#building-on-linux)
  * [Building on Docker](#building-on-docker)
* [Configuration](#configuration)
  * [Options](#options)
  * [Configuring the Environment](#configuring-the-environment)
  * [Configuring Lookback](#configuring-lookback)

## Managing Deployments

The agent has been tested for deployment to Kubernetes 1.9+ and OpenShift 4.6+ environments. 

### Installing

* [Installing Kubernetes](KUBERNETES.md#installing)
* [Installing OpenShift](OPENSHIFT.md#installing)

### Upgrading

* [Upgrading Kubernetes](KUBERNETES.md#upgrading)
* [Upgrading OpenShift](OPENSHIFT.md#upgrading)

### Uninstalling

* [Uninstalling Kubernetes](KUBERNETES.md#uninstalling)
* [Uninstalling OpenShift](OPENSHIFT.md#uninstalling)

### More Information

More information about managing your deployments is documented for [Kubernetes](KUBERNETES.md) or [OpenShift](OPENSHIFT.md). This includes topics such as

* Version specific upgrade paths
* Running the agent as a non-root user
* Collecting node logs through Journald

## Building

### Building on Linux

The agent requires `v1.42+` of rustc [cargo-make](https://github.com/sagiegurari/cargo-make) to build. If the proper versions of rustc and cargo are installed; then simply run the following command to build the agent:

```
cargo build
```

The compiled binary will be built to `./target/release/logdna-agent`.

### Building on Docker

To build a Docker image of the agent, ensure docker is installed properly, verify the docker engine is running, and then run the following command:

```
make build-image
```

The resulting image can be found by listing the images:

```console
foo@bar:~$ docker images
REPOSITORY          TAG                 IMAGE ID            CREATED             SIZE
logdna-agent-v2     dcd54a0             e471b3d8a409        22 seconds ago      135MB
```

## Configuration

### Options

The agent accepts configuration from two sources, environment variables and a configuration yaml. The default configuration yaml location is `/etc/logdna/config.yaml`. The following options are available:

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
|`LOGDNA_LOG_K8S_EVENTS`|Whether the agent shoudl capture Kubernetes Events|`always`|

1. We support [this flavor of globber syntax](https://github.com/CJP10/globber).

### Configuring the Environment

To configure the DaemonSet, make modifications to the envs section of the DameonSet [`spec.template.spec.containers.0.env`]. For example, to change the hostname add the following environment variable to the `env` list:

```yaml
env:
  - name: LOGDNA_HOSTNAME
    value: my-hostname
```

Check out [Kubernetes documentation](https://kubernetes.io/docs/tasks/inject-data-application/define-environment-variable-container/) for more information about injecting environment variables into applications!

### Configuring Lookback

The lookback strategy determines how the agent handles existing container logs on startup. This strategy is determined by `LOGDNA_LOOKBACK`. There's a limited set of valid values for this option:

* `start` - Always start at the beginning of the files.
* `smallfiles` - If the file is less than 8KiB, start at the beginning. Otherwise, start at the end.
* `none` - Always start at the end of the files.
* __Note:__ The default option is `smallfiles`

### Configuring Journald

If the agent pods have access to journald log files or directories, monitoring can be enabled on them with the `LOGDNA_JOURNALD_PATHS`. Common values include `/var/log/journald` and `/run/systemd/journal`. To specify both, use a comma separated list: `/var/log/journald,/run/systemd/journal`.

Take a look at enabling journald monitoring for [Kubernetes](KUBERNETES.md#collecting-node-journald-logs) or [OpenShift](OPENSHIFT.md#collecting-node-journald-logs).

### Configuring Events

Kubernetes and OpenShift Events are by default automatically captured by the agent. This feature can be controlled by the `LOGDNA_LOG_K8S_EVENTS` option with only two valid values:

* `always` - Always capture Events
* `never` - Never capture Events
* __Note:__ The default option is `always`

> :warning: Due to a number of issues with Kubernetes, the agent collects events from the entire cluster including multiple nodes. To prevent duplicate logs when running multiple pods, the agents defer responsibilty of capturing Events to the oldest pod in the cluster. If that pod is killed, the next oldest pod will take over responsibility and continue from where the previous pod left off.
