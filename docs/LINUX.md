# LogDNA Agent on Linux distributions

The LogDNA Agent for Linux collects log data from your Linux environment. The agent is available as a Linux package for Debian-based distributions using `apt` and RPM-based distributions using `yum`. If you use a different package manager that does not support `.deb` or `.rpm` files, you can still use the agent by [manually compiling the binary](README.md#building-agent-binary-on-linux).

## Table of Contents
  * [Considerations](#considerations)
  * [Installation: fresh installs](#Installation)
  * [Upgrading: migrating from legacy Linux agent](#upgrademigration)
  * [Usage](#usage)
    * [Enable the agent](#enable-the-agent)
    * [Configure the agent](#configure-the-agent)
    * [Run the agent](#run-the-agent)

## Considerations
* The agent provides a "stateful" or persistent set of files that is available for reference whenever the agent is restarted; this allows for a configurable `lookback` option. For details, refer to our documentation about [configuring lookback](README.md/#configuring-lookback) and the [configuration options](README.md/#options) for environment variables.

## Installation

1. To download the agent and resources package, open a host terminal and run the appropriate commands based on the Linux distribution.

* **Debian-based distributions**
```bash
echo "deb https://assets.logdna.com stable main" | sudo tee /etc/apt/sources.list.d/logdna.list
wget -qO - https://assets.logdna.com/logdna.gpg | sudo apt-key add -
sudo apt-get update
```
* **RPM-based distributions**
```bash
sudo rpm --import https://assets.logdna.com/logdna.gpg
echo "[logdna]
name=LogDNA packages
baseurl=https://assets.logdna.com/el6/
enabled=1
gpgcheck=1
gpgkey=https://assets.logdna.com/logdna.gpg" | sudo tee /etc/yum.repos.d/logdna.repo
```

2. To install the agent, use the commands below, based on the Linux distribution:

* **Debian-based distributions:**
```bash
sudo apt-get install logdna-agent
```
* **RPM-based distributions:**
```bash
sudo yum install logdna-agent
```

## Upgrade/migration

---
**NOTE**
for users migrating from the [legacy LogDNA Agent](https://github.com/logdna/logdna-agent): If you have previously installed the `logdna-agent` Linux package and have an existing `/etc/logdna.conf` file, it is _still_ recommended to follow the instructions below to ensure that all users are retrieving the latest version of the package from the correct source repository.

---

1.  To download the agent and resources package, open a host terminal and run the appropriate commands based on the Linux distribution.

* **Debian-based distributions:**
```bash
echo "deb https://assets.logdna.com stable main" | sudo tee /etc/apt/sources.list.d/logdna.list
wget -qO - https://assets.logdna.com/logdna.gpg | sudo apt-key add -
sudo apt-get update
```

* **RPM-based distributions:**
```bash
sudo rpm --import https://assets.logdna.com/logdna.gpg
echo "[logdna]
name=LogDNA packages
baseurl=https://assets.logdna.com/el6/
enabled=1
gpgcheck=1
gpgkey=https://assets.logdna.com/logdna.gpg" | sudo tee /etc/yum.repos.d/logdna.repo
```

2. To upgrade the agent, use the commands below, based on the distro:

* **Debian-based distributions:**
```bash
sudo apt-get upgrade logdna-agent
```
* **RPM-based distributions:**
```bash
sudo yum update logdna-agent
```

## Usage
The agent uses [**systemd**](https://systemd.io/) to run as a Linux daemon. The installed package provides a **systemd** unit file for the `logdna-agent` service, which is defined to execute a daemon process with the compiled agent binary. The process that is started in the service is managed by **systemd** and can be interfaced with using `systemctl`.

### _Enable the agent_

1.  Reload the **systemd** unit files to obtain the most recent version of the agent:
```bash
    sudo systemctl daemon-reload
```

2.  Enable the `logdna-agent` service using the `systemctl` command:
```bash
    sudo systemctl enable logdna-agent
```


### _Configure the agent_

---
**NOTE** for users upgrading /migrating from the [legacy LogDNA
Agent](https://github.com/logdna/logdna-agent)\: You might already have a configuration file `/etc/logdna.conf` from prior installations. The LogDNA Agent 3.3 does support the legacy `/etc/logdna.conf` file by default, and additionally uses a **systemd** unit file `/etc/logdna.env`.

---

1.  Create the agent's configuration file (`logdna.env`) in the `/etc` directory, using the following command:

    ```bash
    sudo touch /etc/logdna.env
    ```
    The logdna.env file can be initialized as an empty file; you will add to it in the steps below. This file stores key-value pairs as environment variables that are injected into the agent at runtime and manage a variety of configuration options.

2.  Edit the `/etc/logdna.env` file and set the `LOGDNA_INGESTION_KEY` variable:
    ```bash
    LOGDNA_INGESTION_KEY=<YOUR INGESTION KEY HERE>
    ```

3. (_Optional_) The ingestion key is the only required variable, but you can set
    any additional variables in order to meet your desired
    configuration. For example, to attach a "production" tag to
    every log line, set the `LOGDNA_TAGS` variable in the
    `/etc/logdna.env` file:

    ```bash
    LOGDNA_TAGS=production
    ```
   You can see all the available variable options by running the command `logdna-agent --help` or refer to them in our [README](https://github.com/logdna/logdna-agent-v2/blob/eb06d4f3f7c1033b494f1f0439957f96533f9225/docs/README.md#options). If you're migrating from the legacy agent, take note of the variables that have are changed, updated, and deprecated when compared to the [legacy LogDNA
Agent](https://github.com/logdna/logdna-agent).


### _Run the agent_

1.  After you have added your ingestion key and configured your desired
    settings, save the `/etc/logdna.env` file and then start the
    `logdna-agent` service using the `systemctl` command:

    ```bash
    sudo systemctl start logdna-agent
    ```

2.  Verify that the agent is running and log data flowing, using the
    `systemctl` command to check the status of the agent:
    ```bash
    systemctl status logdna-agent
    ```
