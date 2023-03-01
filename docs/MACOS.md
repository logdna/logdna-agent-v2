# Mezmo Agent on MACOS

The Mezmo Agent for mac collects log data from your mac environment. The agent is available on brew. 

## Installation (first-time installations)

run brew install <> 

The installation process will also configure launchctl so you can run the agent via 
sudo launchctl load -w /Library/LaunchDaemons/com.logdna.logdna-agent.plist

Configuration can be done via /tc/lognda.config file or setting environment variables - MEZMO_INGESTION_KEY and LOGDNA_HOST must be set.  

## Upgrading (migrating from legacy mac agent)

For a clean upgrade brew uninstall the old macos LogDNA and modify the /etc/logdna.config file's logdir from /var/log into /private/var/log
