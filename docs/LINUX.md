# LogDNA Agent on Linux distributions

The agent is available as a Linux package for Debian-based distributions
using apt and Red Hat-based distributions using yum. If you use a
different package manager that does not support .deb or .rpm files, you
can still use the agent by following the steps
[here](https://github.com/logdna/logdna-agent-v2/blob/eb06d4f3f7c1033b494f1f0439957f96533f9225/docs/README.md#building-agent-binary-on-linux)
to manually compile its binary.

## Installation
└ **Note** [for users upgrading from older versions of the [LogDNA Agent](https://github.com/logdna/logdna-agent-v2) or migrating from the [Legacy LogDNA Agent](https://github.com/logdna/logdna-agent)]:
> Even if you have previously installed the `logdna-agent` Linux package, it is _still_ recommended to follow the appropriate instructions below to ensure that all users are retrieving the latest version of the package from the correct logdna source repository.


### _On Debian-based distributions_
```bash
# add logdna source repo → retrieve latest .deb file & manage package via apt
echo "deb https://assets.logdna.com stable main" | sudo tee /etc/apt/sources.list.d/logdna.list
wget -qO - https://assets.logdna.com/logdna.gpg | sudo apt-key add -
sudo apt-get update

# install the package via apt (skip this step if already installed)
sudo apt-get install logdna-agent

# update the package via apt
sudo apt-get upgrade logdna-agent
```

### _On Red Hat-based distributions_
```bash
# import latest .rpm file from logdna source repo → move into yum dir & manage package via yum
sudo rpm --import https://assets.logdna.com/logdna.gpg
echo "[logdna]
name=LogDNA packages
baseurl=https://assets.logdna.com/el6/
enabled=1
gpgcheck=1
gpgkey=https://assets.logdna.com/logdna.gpg" | sudo tee /etc/yum.repos.d/logdna.repo

# install the package via yum (skip this step if already installed)
sudo yum -y install logdna-agent

# update the package via yum
sudo yum update logdna-agent
```

## Usage

The agent uses [**systemd**](https://systemd.io/) to run as a Linux daemon. The installed package provides a **systemd** unit file for the `logdna-agent` service, which is defined to execute a daemon process with the compiled agent binary. The process that is started in the service is managed by **systemd** and can be interfaced with using `systemctl`.

---
### _Enable the agent_

1.  Reload the **systemd** unit files to ensure that the most recent version of the agent is used:
```bash
    sudo systemctl daemon-reload
```

2.  Enable the `logdna-agent` service using the `systemctl` command:
```bash
    sudo systemctl enable logdna-agent
```
---
### _Configure the agent_

└ Note \[for users upgrading /migrating from the [legacy LogDNA
Agent](https://github.com/logdna/logdna-agent)\]: Many users will likely
already have a configuration file `/etc/logdna.conf` from prior
installations. While the config variable names have been changed /
updated between the [Legacy LogDNA
Agent](https://github.com/logdna/logdna-agent) and the current [LogDNA
Agent](https://github.com/logdna/logdna-agent-v2), backward
compatibility has been maintained so that older `/etc/logdna.conf` files
are still valid. If this is applicable to you and there are no changes
that need to be made, you can skip this section and start the service
using your existing configuration.

1.  Create the agent's configuration file (`logdna.env`) in the ``/etc` directory, using the following command:

    ```bash
    sudo touch /etc/logdna.env # it can initialized as an empty file, you will be adding to it in the steps below
    ```
    This file stores key-value pairs as environment variables that
    are injected into the agent at runtime and manage a variety of
    configuration options. You can see all the available variable
    options via `logdna-agent --help` or refer to the table in the
    [README.md file](https://github.com/logdna/logdna-agent-v2/blob/eb06d4f3f7c1033b494f1f0439957f96533f9225/docs/README.md#options).
    If you're migrating from the legacy agent, take note of the
    variables which have been changed, updated, and deprecated when
    compared to the [legacy LogDNA
    Agent](https://github.com/logdna/logdna-agent).

2.  Edit the `/etc/logdna.env` file and set the `LOGDNA_INGESTION_KEY` variable:
    ```bash
    LOGDNA_INGESTION_KEY=<YOUR INGESTION KEY HERE>
    ```

    The ingestion key is the only required variable, but you can set
    any additional variables in order to meet your desired
    configuration. For example, to attach a "production" tag to
    every log line, set the `LOGDNA_TAGS` variable in the
    `/etc/logdna.env` file:

    ```bash
    LOGDNA_INGESTION_KEY=<YOUR INGESTION KEY HERE>
    LOGDNA_TAGS=production
    ```
---
### _Run the agent_

1.  After you have added your ingestion key and configured your desired
    settings, save the `/etc/logdna.env file` and then start the
    `logdna-agent` service using the `systemctl` command:

    ```bash
    sudo systemctl start logdna-agent
    ```

2.  Verify that the agent is running and log data flowing, using the
    `systemctl` command to check the status of the agent:
    ```bash
    systemctl status logdna-agent
    ```
