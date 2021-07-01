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

## Usage

The agent uses [systemd](https://systemd.io/) to run as a Linux daemon.

After installing the package, you should enable it on systemd and set the ingestion key:

```shell script
systemctl daemon-reload
systemctl enable logdna-agent
systemctl edit logdna-agent
```

The command `systemctl edit logdna-agent` will start your default text editor with an empty systemd configuration file
for the LogDNA Agent. You can specify the [ingestion key][ingestion-key] by setting the `LOGDNA_INGESTION_KEY`
environment variable:

```unit file (systemd)
[Service]
Environment="LOGDNA_INGESTION_KEY=<YOUR INGESTION KEY HERE>"
```

You can see all the available environment variables using `logdna-agent --help`. For example, you can set the tags to
attach to each line using `LOGDNA_TAGS` variable:

```unit file (systemd)
[Service]
Environment="LOGDNA_INGESTION_KEY=<YOUR INGESTION KEY HERE>"
Environment="LOGDNA_TAGS=production"
```

After saving the configuration, you can start the service:

```shell script
systemctl start logdna-agent
```

You can check the status of the agent using `systemctl status`:

```shell script
systemctl status logdna-agent
```

## Upgrading from LogDNA Agent v1/v2 for Linux

LogDNA Agent v1/v2 for Linux used initd to start as a daemon and `.conf` files to define the settings.

To upgrade, make sure you use the new repository on `https://assets.logdna.com` and run the upgrade command
of your package manager:

### On Debian-based distributions

```shell script
# Add the new repository
echo "deb https://assets.logdna.com stable main" | sudo tee /etc/apt/sources.list.d/logdna.list
wget -qO - https://assets.logdna.com/logdna.gpg | sudo apt-key add -
sudo apt-get update

# Update the package
sudo apt-get upgrade logdna-agent
```

### On RPM-based distributions

```shell script
# Add the new repository
sudo rpm --import https://assets.logdna.com/logdna.gpg
echo "[logdna]
name=LogDNA packages
baseurl=https://assets.logdna.com/el6/
enabled=1
gpgcheck=1
gpgkey=https://assets.logdna.com/logdna.gpg" | sudo tee /etc/yum.repos.d/logdna.repo

# Update the package
sudo yum update logdna-agent
```

The agent will uninstall the previous version and reuse the existing configuration file, by default
located in `/etc/logdna.conf`.

If you defined a configuration file on a different location, you can specify it on your systemd unit file:

```unit file (systemd)
[Service]
Environment="LOGDNA_CONFIG_FILE=/your/path/to/logdna.conf"
```

Followed by a restart:

```shell script
systemctl restart logdna-agent
```

[ingestion-key]: https://docs.logdna.com/docs/ingestion-key
