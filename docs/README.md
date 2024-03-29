# LogDNA Agent

![agent version](https://img.shields.io/badge/Version-3.7.0-E9FF92.svg)
[![made-with-rust](https://img.shields.io/badge/Made%20with-Rust-orange.svg)](https://www.rust-lang.org/)

Mezmo, formerly LogDNA, enables enterprises to ingest all of their log data to a single platform, normalize it, and route it to the appropriate teams so that they can take meaningful action in real time. Join us at the Mezmo community [forum](https://community.logdna.com).

The LogDNA agent is a resource-efficient log collection client that forwards logs to [LogDNA]. This version of the agent is written in [Rust] to ensure maximum performance, and when coupled with LogDNA's web application, provides a powerful log management tool for distributed systems, including [Kubernetes] clusters. Supported platforms include Linux, Windows and Mac.


<table>
  <thead>
    <tr>
      <td align="left">
        :information_source: Information
      </td>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>
        <ul>
As part of the new company name change, we will be changing the name of our agent to <b>mezmo-agent</b>. This will happen over a series of releases. In <b>3.6</b> we will be introducing environment variables prefixed with <b>MZ_</b> in addition to the existing prefix of <b>LOGDNA_</b>. In <b>3.7</b> we will release both the agent binaries and yaml files with mezmo in the names. Here again we will do this in tandem with the logdna name. In our <b>4.0</b> version, we will fully deprecate all previous references to <b>LOGDNA_</b> and logdna.
        </ul>
      </td>
    </tr>
  </tbody>
</table>


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
  * [Installing on Windows](#installing-on-windows)
* [Building](#building-the-logdna-agent)
  * [Building Docker image](#building-docker-image)
* [Configuration](#configuration)
  * [Options](#options)
  * [Configuring the Environment](#configuring-the-environment)
  * [Configuring Lookback](#configuring-lookback)
  * [Configuring Journald](#configuring-journald)
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

### Installing on Windows

Refer to the documentation for [installing the agent on Windows](WINDOWS.md).

### Installing on MacOS

Refer to the documentation for [installing the agent on MacOS](MACOS.md).


### Running as Non-Root

By default the agent will run as root. Below are environment-specific instructions for running the agent as a non-root user.

* [Running as Non-Root on Kubernetes](KUBERNETES.md#run-as-non-root)
* [Running as Non-Root on OpenShift](OPENSHIFT.md#run-as-non-root)

If you configure the LogDNA Agent to run as non-root, review the [documentation about enabling "statefulness" for the agent](KUBERNETES.md#enabling-persistent-agent-state).

#### Additional Installation Options

More information about managing your deployments is documented for [Kubernetes](KUBERNETES.md) or [OpenShift](OPENSHIFT.md). This includes topics such as

* Version specific upgrade paths
* Collecting system logs through Journald

### Installing on Windows

Refer to the documentation for [installing the agent on Windows](WINDOWS.md).

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

The agent requires rustc [cargo-make](https://github.com/sagiegurari/cargo-make) to build. If rustc and cargo are [installed](https://www.rust-lang.org/tools/install), simply run the following command to build the agent:

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
configuration YAML file. The default configuration yaml file is located at `/etc/logdna/config.yaml`. Unless noted, when an option says *List of*, this would mean a comma separated list for Environment variable and a YAML array list for YAML.

Some variables do not have a corresponding Yaml path because they are only applicable to k8s deployments and therefor would use environment variables.

For backward compatibility agent v1 configuration file format is still supported. For example - upgrade to Agent v2 on Windows can re-use v1 config file from previous installation as-is.  *The format of these v1 configuration parameter names are the same as the environment variables without `LOGDNA_` and in lower case.*

| Environment Variable Name | Yaml Path Name | Description | Default |
| ---|---|---|---|
|`LOGDNA_INGESTION_KEY`<br>**Deprecated**: `LOGDNA_AGENT_KEY`|`http.ingestion_key`|**Required**: The ingestion key associated with your LogDNA account||
|`LOGDNA_CONFIG_FILE`<br>**Deprecated**: `DEFAULT_CONF_FILE`||Path to the configuration yaml|`/etc/logdna/config.yaml`|
|`LOGDNA_HOST`<br>**Deprecated**: `LDLOGHOST`|`http.host`|The host to forward logs to|`logs.logdna.com`|
|`LOGDNA_ENDPOINT`<br>**Deprecated**: `LDLOGPATH`|`http.endpoint`|The endpoint to forward logs to|`/logs/agent`|
|`LOGDNA_USE_SSL`<br>**Deprecated**: `LDLOGSSL`|`http.use_ssl`|Whether to use a SSL for sending logs|`true`|
|`LOGDNA_USE_COMPRESSION`<br>**Deprecated**: `COMPRESS`|`http.use_compression`|Whether to compress logs before sending|`true`|
|`LOGDNA_GZIP_LEVEL`<br>**Deprecated**: `GZIP_COMPRESS_LEVEL`|`http.gzip_level`|If compression is enabled, this is the gzip compression level to use|`2`|
|`LOGDNA_HOSTNAME`|`http.params.hostname`|The hostname metadata to attach to lines forwarded from this agent||
|`LOGDNA_IP`|`http.params.ip`|The IP metadata to attach to lines forwarded from this agent||
|`LOGDNA_TAGS`|`http.params.tags`|Comma separated list of tags metadata to attach to lines forwarded from this agent||
|`LOGDNA_MAC`|`http.params.mac`|The MAC metadata to attach to lines forwarded from this agent||
|`LOGDNA_LOG_DIRS`<br>**Deprecated**: `LOG_DIRS`|`log.dirs[]`|List of folders to recursively monitor for log events|`/var/log/`|
|`LOGDNA_EXCLUSION_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE`|`log.exclude.glob[]`|List of glob patterns to exclude files from monitoring <sup>1</sup>|`/var/log/wtmp,/var/log/btmp,/var/log/utmp,` <br>`/var/log/wtmpx,/var/log/btmpx,/var/log/utmpx,` <br>`/var/log/asl/**,/var/log/sa/**,/var/log/sar*,` <br>`/var/log/tallylog,/var/log/fluentd-buffers/**/*,` <br>`/var/log/pods/**/*`|
|`LOGDNA_EXCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_EXCLUDE_REGEX`|`log.exclude.regex[]`|List of regex patterns to exclude files from monitoring||
|`LOGDNA_INCLUSION_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE`|`log.include.glob[]`|List of glob patterns to includes files for monitoring <sup>1</sup>|`*.log`|
|`LOGDNA_INCLUSION_REGEX_RULES`<br>**Deprecated**: `LOGDNA_INCLUDE_REGEX`|`log.include.regex[]`|List of regex patterns to include files from monitoring||
|`LOGDNA_LINE_EXCLUSION_REGEX`|`log.line_exclusion_regex[]`|List of regex patterns to exclude log lines. When set, the Agent will NOT send log lines that match any of these patterns.||
|`LOGDNA_LINE_INCLUSION_REGEX`|`log.line_inclusion_regex[]`|List of regex patterns to include log lines. When set, the Agent will send ONLY log lines that match any of these patterns.||
|`LOGDNA_REDACT_REGEX`|`log.line_redact_regex`|List of regex patterns used to mask matching sensitive information (such as PII) before sending it in the log line.||
|`LOGDNA_JOURNALD_PATHS`|`journald.paths[]`|List of paths (directories or files) of journald paths to monitor||
|`MZ_SYSTEMD_JOURNAL_TAILER`||True/False toggles journald on the agent|'true'|
|`LOGDNA_LOOKBACK`|`log.lookback`|The lookback strategy on startup|`none`|
|`LOGDNA_K8S_STARTUP_LEASE`||Determines whether or not to use K8 leases on startup|`never`|
|`LOGDNA_USE_K8S_LOG_ENRICHMENT`||Determines whether the agent should query the K8s API to enrich log lines from other pods.|`always`|
|`LOGDNA_LOG_K8S_EVENTS`||Determines whether the agent should log Kubernetes resource events. This setting only affects tracking and logging Kubernetes resource changes via watches. When disabled, the agent may still query k8s metadata to enrich log lines from other pods depending on the value of `LOGDNA_USE_K8S_LOG_ENRICHMENT` setting value.|`never`|
|`LOGDNA_LOG_METRIC_SERVER_STATS`||Determines whether or not metrics usage statistics is enabled.|`never`|
|`LOGDNA_DB_PATH`|`log.db_path`|The directory in which the agent will store its state database. Note that the agent must have write access to the directory and be a persistent volume.|`/var/lib/logdna`|
|`LOGDNA_METRICS_PORT`|`log.metrics_port`|The port number to expose a Prometheus endpoint target with the [agent internal metrics](INTERNAL_METRICS.md).||
|`LOGDNA_INGEST_TIMEOUT`|`http.timeout`|The timeout of the API calls to the ingest API in milliseconds|`10000`|
|`LOGDNA_INGEST_BUFFER_SIZE`|`http.body_size`|The size, in bytes, of the ingest data buffer used to batch log data with.|`2097152`|
|`LOGDNA_RETRY_DIR`|`http.retry_dir`|The directory used by the agent to store data temporarily while retrying calls to the ingestion API.|`/tmp/logdna`|
|`LOGDNA_RETRY_DISK_LIMIT`|`http.retry_disk_limit`|The maximum amount of disk space the agent will use to store retry data. The value can be the total number of bytes or a human representation of space using suffixes "KB", "MB", "GB" or "TB", e.g. `10 MB` If left unset, the agent will not limit disk usage. If set to `0`, no retry data will be stored on disk.||
|`LOGDNA_META_APP`||Overrides/omits `APP` field in log line metadata. [Examples](META.md)||
|`LOGDNA_META_HOST`||Overrides/omits `HOST` field in log line metadata.||
|`LOGDNA_META_ENV`||Overrides/omits `ENV` field in log line metadata.||
|`LOGDNA_META_FILE`||Overrides/omits `FILE` field in log line metadata.||
|`LOGDNA_META_K8S_FILE`||Overrides/omits `FILE` field in k8s log line metadata. Follows `LOGDNA_META_FILE`.||
|`LOGDNA_META_JSON`||Overrides/omits `META` filed in log line metadata.||
|`LOGDNA_META_ANNOTATIONS`||Overrides specific kay-value-pairs inside `ANNOTATIONS` field in log line metadata.||
|`LOGDNA_META_LABELS`||Overrides specific kay-value-pairs inside `LABELS` field in log line metadata.||
|`MZ_METADATA_RETRY_DELAY`||The number of seconds to wait for Pod label availability before sending log lines.|0|
|`MZ_FLUSH_DURATION`|`http.flush_duration`|The number of milliseconds to wait before flushing captured logs to the ingestion API.|`5000`|


All regular expressions use [Perl-style syntax][regex-syntax] with case sensitivity by default. If you don't
want to differentiate between capital and lower-case letters, use non-capturing groups with a flag: `(?flags:exp)`,
for example:

```
(?i:my_case_insensitive_regex)
```

1. We support [this flavor of globber syntax](https://docs.rs/glob/*/glob/).

As listed above, by default the agent will only ingest files with a `.log` extention. Globing patterns are checked against full file path. Some other common globing patterns to include in the `LOGDNA_INCLUSION_RULES` could be: 

1. Pattern to ingest files with both a `.log` extention AND extention-less files:
  - `*.log,!(*.*)`
2. Applied to specific log dir:
  - `/var/log/containers/mypod*.log`
  - `*/containers/mypod*.log`
3. Applied to all log dirs:
  - `*/app?.log`

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
   * When set to **`tail`**:
      * Will read .log files starting at the beginning of the file, if the .log file has a create timestamp that is later than when the agent began running.  
      * Agent will warn if you are picking up .log files greater than 3MB.  
      

**Notes:**
* If you configure the LogDNA Agent to run as non-root, review the [documentation](KUBERNETES.md#enabling-file-offset-tracking-across-restarts) about enabling "statefulness" for the LogDNA Agent.
* When upgrading from LogDNA Agent version 3.0 to 3.1, the state file will initially be empty, so the lookback setting will be used for existing files. After that (i.e. on process restart), the state file will be present and will be used.

### Configuring Lease Startup

The lease startup configuration uses Kubernetes Leases to limit the number of agents that can start at one time on a cluster. When enabled, the agent will "claim" a lease before starting. Once started, the agent will then release the lease. If no leases are available, the agent will wait for one to become available. This feature would only be needed if running the agent on a cluster large enough that you'd risk crashing `etcd` if all the the agents tried to connect at once.

The valid values for this option are:
* When set to **`never`** (default):
  * The agent will start normally and will not attempt to obtain a lease before starting.
* When set to **`attempt`**:
  * The agent will attempt to connect to a lease before starting, but will start anyway after a number of unsuccessful attempts.
* When set to **`always`**:
  * The agent will attempts to connect to a lease indefinitely.

**Note:**
For more information on how to configure leases on a your Kubernetes cluster, see the "Installation Steps" in the KUBERNETES.md 
file in docs directory of this repository.

### Configuring Journald

You can monitor journald log files or directories that the agent pods have access to by adding them to the `LOGDNA_JOURNALD_PATHS` and setting `MZ_SYSTEMD_JOURNAL_TAILER` to true. Alternatively you leave `MZ_SYSTEMD_JOURNAL_TAILER` blank as it defaults to true. Common values include for `LOGDNA_JOURNALD_PATHS` - `/var/log/journal` and `/run/systemd/journal`. To specify both, use a comma separated list: `/var/log/journal,/run/systemd/journal`.

Take a look at enabling journald monitoring for [Kubernetes](KUBERNETES.md#collecting-node-journald-logs) or [OpenShift](OPENSHIFT.md#collecting-node-journald-logs).

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

The agent requires at least 128Mib of memory and
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
