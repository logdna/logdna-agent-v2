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

* [Upgrading](#upgrading)
    * [Preamble](#preamble)
	* [Upgrading on Kubernetes](#upgrading-on-kubernetes)
* [Installing](#installing)
    * [Prerequisites](#prerequisites)
	* [Installing on Kubernetes](#installing-on-kubernetes)
* [Building](#building)
	* [Building on Linux](#building-on-linux)
	* [Building on Docker](#building-on-docker)
* [Configuration](#configuration)
    * [Options](#options)
    * [Configuring Kubernetes](#configuring-kubernetes)

## Upgrading

### Preamble

The agent comes with configuration files that allow it to be deployed to a number of environments. Often, these configuration files, once set, do not change whereas the image they use can update. This introduces issues where configuration files can fall behind and not provide the up to date features that come with evolving tools. We aim to make changes in the agent backwards compatible with old configuration files, but recommend regularly ensuring your config is up to date. The agent will try to alert users when it can't use new features.

### Upgrading on Kubernetes

We recommend doing a complete reinstall of the agent by removing the existing configuration and installing the latest one.

1. A backup of your existing agent yaml is highly recommended:
```
kubectl get ds logdna-agent -o yaml > old-logdna-agent.yaml
```
2. Delete the existing agent using the yaml you installed with:
```
# 1.x.x
kubectl delete -f https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-ds.yaml

# 2.x.x
kubectl delete -f https://raw.githubusercontent.com/logdna/logdna-agent/master/logdna-agent-v2.yaml
```
3. Follow our instructions for [installing the agent on kubernetes](#installing-on-kubernetes).

## Installing

### Prerequisites

* A LogDNA Account. Create an account with LogDNA by following our [quick start guide](https://docs.logdna.com/docs/logdna-quick-start-guide).
* A LogDNA Ingestion Key. You can get your ingestion key at the top of [your account's Add a Log Source page](https://app.logdna.com/pages/add-host).

### Installing on Kubernetes

The agent is compatible with Kubernetes clusters running `v1.9` or greater. It can be quickly and effortlessly deployed to all nodes to forward logs from your entire kubernetes cluster by running two commands:


```
kubectl create -f https://raw.githubusercontent.com/logdna/logdna-agent-v2/master/k8s/logdna-agent.yaml
kubectl create secret generic logdna-agent-key -n logdna-agent --from-literal=logdna-agent-key=<YOUR LOGDNA INGESTION KEY>
```

## Building

### Building on Linux

The agent requires `v1.42+` of rustc. If the proper versions of rustc and cargo are installed, then simply run the following command to build the agent:

```
cargo build --release
```

The compiled binary will be built to `./target/release/logdna-agent`.

### Building on Docker

To build a Docker image of the agent, ensure docker is installed properly, verify the docker engine is running, and then run the following command:

```
docker build .
```

The resulting image can be found by listing the images:

```console
foo@bar:~$ docker images
REPOSITORY          TAG                 IMAGE ID            CREATED             SIZE
<none>              <none>              f541543bcd7f        64 seconds ago      119MB
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

1. We support [this flavor of globber syntax](https://github.com/CJP10/globber).

### Configuring Kubernetes

To configure the kubernetes daemonset, copy the [logdna-agent yaml](../k8s/logdna-agent.yaml) and modify the `env` section. For example, to change the hostname add the following:

```yaml
env:
  - name: LOGDNA_HOSTNAME
    value: my-hostname
```

Check out [Kubernetes documentation](https://kubernetes.io/docs/tasks/inject-data-application/define-environment-variable-container/) for more information about injecting environment variables into applications!
