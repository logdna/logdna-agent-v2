# LogDNA Agent

[![Rustc Version 1.46+]][rustc]
[Join us at the LogDNA community forum]: https://community.logdna.com

The LogDNA agent is a resource-efficient log collection client that forwards logs to [LogDNA]. This version of the agent is written in [Rust] to ensure maximum performance, and when coupled with LogDNA's web application, provides a powerful log management tool for distributed systems, including [Kubernetes] clusters.

[Rustc Version 1.46+]: https://img.shields.io/badge/rustc-1.42+-lightgray.svg
[rustc]: https://blog.rust-lang.org/2020/03/12/Rust-1.42.html
[Join us at the LogDNA community forum]: https://community.logdna.com
[LogDNA]: https://logdna.com
[Rust]: https://www.rust-lang.org/
[Kubernetes]: https://kubernetes.io/

## Table of Contents

* [Managing Deployments](#managing-deployments)
  * [Installing on Kubernetes](#installing-on-kubernetes)
    * [Using manifest files](#using-manifest-files)
    * [Using Helm](#using-helm)
  * [Installing on OpenShift](#installing-on-openshift)
  * [Installing on Linux](#installing-on-linux)
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
  * [Configuring regex for redaction and exclusion or inclusion](#configuring-regex-for-redaction-and-exclusion-or-inclusion)
  * [Resource Limits](#resource-limits)
  * [Exposing Agent Metrics](#exposing-agent-metrics)

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

Follow the instructions for [deploying the agent on Red Hat OpenShift](OPENSHIFT.md).

### Installing on Linux

Refer to the documentation for [deploying the agent on Linux](LINUX.md).

### Running as Non-Root

By default the agent will run as root. Below are environment-specific instructions for running the agent as a non-root user.

* [Running as Non-Root on Kubernetes](KUBERNETES.md#run-as-non-root)
* [Running as Non-Root on OpenShift](OPENSHIFT.md#run-as-non-root)

If you configure the LogDNA Agent to run as non-root, review the [documentation about enabling "statefulness" for the agent](KUBERNETES.md#enabling-persistent-agent-state).

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


### Helper scripts
Some make helper scripts are located under `./scripts` folder:
```
* mk                    make "build" target - build Agent from rust sources
* mk.lint               make "lint" target - run linting
* mk.test               make "test" target - run unit tests
* mk.integration_test   make "integration_test" target - run integration tests
* mk.image              make "image" target - create Agent container image and publish it in local docker
```
#### Note:
- all targets are using "rust-xxxx" container image from "logdna/build-images" registry, will try to use local image first.
- to build arm64 image use:
```
$ ARCH=aarch64 scripts/mk.image
```
- "multi-arch build" in docker needs to be installed and configured before building arm64 on x86 platforms


## Configuration

### Options

The agent accepts configuration from three different sources: environment variables, command line arguments and/or a
configuration YAML file. The default configuration yaml file is located at `/etc/logdna/config.yaml`. The following
options are available:

| Variable Name(s) | Description | Default |
| ---|---|---|
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
|`LOGDNA_EXCLUSION_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE`|Comma separated list of glob patterns to exclude files from monitoring <sup>1</sup>|`/var/log/wtmp,/var/log/btmp,/var/log/utmp,` <br>`/var/log/wtmpx,/var/log/btmpx,/var/log/utmpx,` <br>`/var/log/asl/**,/var/log/sa/**,/var/log/sar*,` <br>`/var/log/tallylog,/var/log/fluentd-buffers/**/*,` <br>`/var/log/pods/**/*`|
|`LOGDNA_EXCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE_REGEX`|Comma separated list of regex patterns to exclude files from monitoring||
|`LOGDNA_INCLUSION_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE`|Comma separated list of glob patterns to includes files for monitoring <sup>1</sup>|`*.log`|
|`LOGDNA_INCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE_REGEX`|Comma separated list of regex patterns to include files from monitoring||
|`LOGDNA_LINE_EXCLUSION_REGEX`|Comma separated list of regex patterns to exclude log lines. When set, the Agent will NOT send log lines that match any of these patterns.||
|`LOGDNA_LINE_INCLUSION_REGEX`|Comma separated list of regex patterns to include log lines. When set, the Agent will send ONLY log lines that match any of these patterns.||
|`LOGDNA_REDACT_REGEX`|Comma separated list of regex patterns used to mask matching sensitive information (such as PII) before sending it in the log line.||
|`LOGDNA_JOURNALD_PATHS`|Comma separated list of paths (directories or files) of journald paths to monitor||
|`LOGDNA_LOOKBACK`|The lookback strategy on startup|`none`|
|`LOGDNA_USE_K8S_LOG_ENRICHMENT`|Determines whether the agent should query the K8s API to enrich log lines from other pods.|`always`|
|`LOGDNA_LOG_K8S_EVENTS`|Determines whether the agent should log Kubernetes resource events. This setting only affects tracking and logging Kubernetes resource changes via watches. When disabled, the agent may still query k8s metadata to enrich log lines from other pods depending on the value of `LOGDNA_USE_K8S_LOG_ENRICHMENT` setting value.|`never`|
|`LOGDNA_DB_PATH`|The directory in which the agent will store its state database. Note that the agent must have write access to the directory and be a persistent volume.|`/var/lib/logdna`|
|`LOGDNA_METRICS_PORT`|The port number to expose a Prometheus endpoint target with the [agent internal metrics](INTERNAL_METRICS.md).||
|`LOGDNA_INGEST_TIMEOUT`|The timeout of the API calls to the ingest API in milliseconds|`10000`|
|`LOGDNA_INGEST_BUFFER_SIZE`|The size, in bytes, of the ingest data buffer used to batch log data with.|`2097152`|
|`LOGDNA_RETRY_DIR`|The directory used by the agent to store data temporarily while retrying calls to the ingestion API.|`/tmp/logdna`|
|`LOGDNA_RETRY_DISK_LIMIT`|The maximum amount of disk space the agent will use to store retry data. The value can be the total number of bytes or a human representation of space using suffixes "KB", "MB", "GB" or "TB", e.g. `10 MB` If left unset, the agent will not limit disk usage. If set to `0`, no retry data will be stored on disk.||

All regular expressions use [Perl-style syntax][regex-syntax] with case sensitivity by default. If you don't
want to differentiate between capital and lower-case letters, use non-capturing groups with a flag: `(?flags:exp)`,
for example:

```
(?i:my_case_insensitive_regex)
```

1. We support [this flavor of globber syntax](https://github.com/CJP10/globber).

As listed above, by default the agent will only ingest files with a `.log` extention. Some other common globing patterns used in the `LOGDNA_INCLUSION_RULES` include: 

1. Include both `.log` extention AND extention-less files: `*.log,!(*.*)`.

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

By default, the agent provides a "stateful", or persistent, collection of files that can be referenced whenever the agent is restarted, in order to return (or look back) to see any files that were ingested during the time that the agent was not running. The state directory location is defined using the `LOGDNA_DB_PATH` environment variable in the YAML file (the default path is `/var/lib/logdna`).

The valid values for this option are:
   * When set to **`none`** (default):
      * lookback is disabled, and LogDNA Agent will read new lines as those are added to the file, ignoring the lines that were written before the time the Agent restarted.
   * When set to **`smallfiles`**:
       * If there is information in the “state file”, use the last recorded state. 
       * If the file is not present in the “state file” and the file is less than 8KiB, start at the beginning. If the file is larger than 8KiB, start at the end. 
   * When set to **`start`**:
      * If there is information in the “state file”, use the last recorded state. 
      * If the file is not present in the “state file”, start at the beginning. 

**Notes:**
* If you configure the LogDNA Agent to run as non-root, review the [documentation](KUBERNETES.md#enabling-file-offset-tracking-across-restarts) about enabling "statefulness" for the LogDNA Agent.
* When upgrading from LogDNA Agent version 3.0 to 3.1, the state file will initially be empty, so the lookback setting will be used for existing files. After that (i.e. on process restart), the state file will be present and will be used.


### Configuring Journald

If the agent pods have access to journald log files or directories, monitoring can be enabled on them with the `LOGDNA_JOURNALD_PATHS`. Common values include `/var/log/journal` and `/run/systemd/journal`. To specify both, use a comma separated list: `/var/log/journal,/run/systemd/journal`.

Take a look at enabling journald monitoring for [Kubernetes](KUBERNETES.md#collecting-node-journald-logs) or [OpenShift](OPENSHIFT.md#collecting-node-journald-logs).

### Configuring Events

A Kubernetes event is exactly what it sounds like: a resource type that is automatically generated when state changes occur in other resources, or when errors or other messages manifest across the system. Monitoring events is useful for debugging your Kubernetes cluster.

By default, the LogDNA agent captures Kubernetes events (and OpenShift events, as well, since OpenShift is built on top of Kubernetes clusters).

To control whether the LogDNA agent collects Kubernetes events, configure the `LOGDNA_LOG_K8S_EVENTS` environment variable using on of these two values:

* `always` - Always capture events
* `never` - Never capture events
__Note:__ The default option is `never`.

> :warning: Due to a ["won't fix" bug in the Kubernetes API](https://github.com/kubernetes/kubernetes/issues/41743), the LogDNA agent collects events from the entire cluster, including multiple nodes. To prevent duplicate logs when running multiple pods, the LogDNA agent pods defer responsibilty of capturing events to the oldest pod in the cluster. If that pod is down, the next oldest LogDNA agent pod will take over responsibility and continue from where the previous pod left off.

### Configuring regex for redaction and exclusion or inclusion

You can define rules, using regex (regular expressions), to control what log data is collected by the agent and forwarded to LogDNA.

For example, you can identify certain types of logs that are noisy and not needed at all, and then write a regex pattern to match those lines and preclude them from collection. Conversely, you can use regex expressions to match the log lines that you do want to include, and collect **only** those log lines.

Additionally, you can use the environment variable `LOGDNA_REDACT_REGEX` to remove certain parts of a log line. Any redacted data is replaced with [REDACTED]. Redacting information is recommended when logs might contain PII (Personally Identifiable Information).

In general, using Agent-level environment variables to define what log data does not ever leave the data center or environment allows a higher level of control over sensitive data, and can also reduce any potential data egress fees.

You can use regex pattern matching via environment variables to:

* include *only* specific log lines (with `LOGDNA_LINE_INCLUSION_REGEX`)
* exclude specific log lines (with `LOGDNA_LINE_EXCLUSION_REGEX`)
* redact parts of a log line (with `LOGDNA_REDACT_REGEX`)

To access our library of common regex patterns refer to [our regex library documentation](REGEX.md).

Notes:

* Exclusion rules are applied after inclusion rules, and thus override inclusion rules. That is, for a line to be ingested, it should match all inclusion rules (if any) and not match any exclusion rule.
* Note that we use commas as separators for environment variable values, making it not possible to use the comma character (,) as a valid value. We are addressing this limitation in upcoming versions. If you need to use the comma character in a regular expression, use the unicode character reference: `\u002C`, for example: `hello\u002C world` matches `hello, world`.
* All regular expressions are case-sensitive by default. If you don't want to differentiate between upper and lower-case letters, use non-capturing groups with a flag: `(?flags:exp)`, for example: `(?i:my_case_insensitive_regex)`
* LogDNA also provides post-ingestion <a href="https://docs.logdna.com/docs/excluding-log-lines" target="_blank">exclusion rules</a> to control what log data is displayed and stored in LogDNA.

### Resource Limits

The agent is deployed as a Kubernetes DaemonSet, creating one pod per node selected. The agent collects logs of all
the pods in the node. The resource requirements of the agent are in direct relation to the amount of pods per node,
and the amount of logs producer per pod.

The agent requires at least 128Mib and no more than 512Mib of memory. It requires at least
[twenty millicpu (`20m`)][k8s-cpu-usage].

Different features can also increase resource utilization. When line exclusion/inclusion or redaction rules
are specified, you can expect to additional CPU consumption per line and per regex rule defined. When Kubernetes
event logging is enabled (disabled by default), additional CPU usage will occur on the oldest agent pod.

We do not recommend placing traffic shaping or CPU limits on the agent to ensure data can be sent to our
log ingestion service.

### Exposing Agent Metrics

The LogDNA agent records internal metrics that can be relevant for monitoring and alerting, such as number log
files currently tracked or number of bytes parsed, along with process status information. Check out the
[documentation for internal metrics](INTERNAL_METRICS.md) for more information.

[regex-syntax]: https://docs.rs/regex/1.4.5/regex/#syntax
[k8s-cpu-usage]: https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/#meaning-of-cpu
