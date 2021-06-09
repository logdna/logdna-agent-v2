# LogDNA Agent on Linux distributions

The agent supports 64-bit (`x86_64`) Linux distributions and provides rpm and deb packages.

## Installing

### On Debian-based distributions

```shell script
echo "deb https://assets.logdna.com stable main" | sudo tee /etc/apt/sources.list.d/logdna.list
wget -qO - https://assets.logdna.com/logdna.gpg | sudo apt-key add -
sudo apt-get update
sudo apt-get install logdna-agent
```

### On RPM-based distributions

```shell script
sudo rpm --import https://assets.logdna.com/logdna.gpg
echo "[logdna]
name=LogDNA packages
baseurl=https://assets.logdna.com/el6/
enabled=1
gpgcheck=1
gpgkey=https://assets.logdna.com/logdna.gpg" | sudo tee /etc/yum.repos.d/logdna.repo
sudo yum -y install logdna-agent
```

# Usage

You can start sending logs by setting the [ingestion key][ingestion-key]:

```shell script
logdna-agent -k <YOUR INGESTION KEY>
```

The agent expose short argument abbreviations for commonly used options (`-k`, `-t`, `-d` and `-c`).

You can use `--help` flag to list all the command and environment variable options. For example with agent
version `3.3.0`:

```shell script
$ logdna-agent --help
LogDNA Agent 3.3.0
A resource-efficient log collection agent that forwards logs to LogDNA.

USAGE:
    logdna-agent [OPTIONS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -c, --config <config>
            The config filename [env: LOGDNA_CONFIG_FILE=]  [default: /etc/logdna/config.yaml]

        --db-path <db-path>
            The directory the agent will store it's state database. Note that the agent must have write access to the
            directory and be a persistent volume. Defaults to "/var/lib/logdna-agent/" [env: LOGDNA_DB_PATH=]
        --endpoint-path <endpoint-path>
            The endpoint to forward logs to. Defaults to "/logs/agent" [env: LOGDNA_ENDPOINT=]
        --exclude-regex <exclusion-regex>...
            List of regex patterns to exclude files from monitoring [env: LOGDNA_EXCLUSION_REGEX_RULES=]
        --exclude <exclusion-rules>...
            List of glob patterns to exclude files from monitoring, to add to the default [env: LOGDNA_EXCLUSION_RULES=]
        --gzip-level <gzip-level>
            If compression is enabled, this is the gzip compression level to use. Defaults to 2 [env:
            LOGDNA_GZIP_LEVEL=]
        --host <host>
            The host to forward logs to. Defaults to "logs.logdna.com" [env: LOGDNA_HOST=]
        --include-regex <inclusion-regex>...
            List of regex patterns to include files from monitoring [env: LOGDNA_INCLUSION_REGEX_RULES=]
        --include <inclusion-rules>...
            List of glob patterns to includes files for monitoring, to add to the default (*.log) [env:
            LOGDNA_INCLUSION_RULES=]
        --ip <ip>
            The IP metadata to attach to lines forwarded from this agent [env: LOGDNA_IP=]
        --journald-paths <journald-paths>...
            List of paths (directories or files) of journald paths to monitor, for example: /var/log/journal or
            /run/systemd/journal [env: LOGDNA_JOURNALD_PATHS=]
    -k, --key <key>
            The ingestion key associated with your LogDNA account [env: LOGDNA_INGESTION_KEY=]
        --line-exclusion <line-exclusion>...
            List of regex patterns to exclude log lines. When set, the Agent will NOT send log lines that match any of
            these patterns [env: LOGDNA_LINE_EXCLUSION_REGEX=]
        --line-inclusion <line-inclusion>...
            List of regex patterns to include log lines. When set, the Agent will ONLY send log lines that match any of
            these patterns [env: LOGDNA_LINE_INCLUSION_REGEX=]
        --line-redact <line-redact>...
            List of regex patterns used to mask matching sensitive information before sending it the log line [env:
            LOGDNA_REDACT_REGEX=]
    -d, --logdir <log-dirs>...
            Adds log directories to scan, in addition to the default (/var/log) [env: LOGDNA_LOG_DIRS=]
        --log-k8s-events <log-k8s-events>
            Whether the agent should log Kubernetes resource events. This setting only affects tracking and logging
            Kubernetes resource changes via watches. When disabled, the agent may still query k8s metadata to enrich log
            lines from other pods depending on the value of `use_k8s_enrichment` setting value ("always" or "never").
            Defaults to "never" [env: LOGDNA_LOG_K8S_EVENTS=]
        --lookback <lookback>
            The lookback strategy on startup ("smallfiles", "start" or "none"). Defaults to "smallfiles" [env:
            LOGDNA_LOOKBACK=]
        --mac-address <mac>
            The MAC metadata to attach to lines forwarded from this agent [env: LOGDNA_MAC=]
        --metrics-port <metrics-port>
            The port number to expose a Prometheus endpoint target with the agent metrics [env: LOGDNA_METRICS_PORT=]
        --os-hostname <os-hostname>
            The hostname metadata to attach to lines forwarded from this agent (defaults to os.hostname()) [env:
            LOGDNA_HOSTNAME=]
    -t, --tags <tags>...
            List of tags metadata to attach to lines forwarded from this agent [env: LOGDNA_TAGS=]
        --use-compression <use-compression>
            Whether to compress logs before sending. Defaults to "true" [env: LOGDNA_USE_COMPRESSION=]
        --use-k8s-enrichment <use-k8s-enrichment>
            Determines whether the agent should query the K8s API to enrich log lines from other pods ("always" or
            "never").  Defaults to "always" [env: LOGDNA_USE_K8S_LOG_ENRICHMENT=]
        --use-ssl <use-ssl>
            Whether to use TLS for sending logs. Defaults to "true" [env: LOGDNA_USE_SSL=]
```

[ingestion-key]: https://docs.logdna.com/docs/ingestion-key
