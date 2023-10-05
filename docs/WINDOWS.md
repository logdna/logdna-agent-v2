# Mezmo Agent for Windows

In addition to the standard log files tailing, the agent on Windows supports streaming of Windows Event Logs which has the following features:
* Live tail - following new events
* Lookback
* XPath queries
* Event Transforms - applying user defined transformations to events
* Keeping track of last tailed events - maintaining persistent state between runs
* Windows service mode with self install and uninstall
* Custom tail output using standard logging framework
* Using fast native libxml2 (lxml) for XML parsing and XSLT transforms (JSON assembly)
* Integration with Mezmo Agent

The Windows Event logs support is implemented in [Windows Event Log Tailer](https://github.com/logdna/winevt-tailer), a separate tool provided by Mezmo, bundled and integrated with the agent using Mezmo Agent Tailer API.


## Installation

- Installation directories and files:

  ```
  C:\Program Files\Mezmo               - application files
  C:\ProgramData\logdna\logdna.conf    - Agent configuration file
  C:\ProgramData\logdna\agent_state.db - location of log offsets database [LOGDNA_DB_PATH]
  C:\ProgramData\logs                  - log files directory monitored by Agent [LOGDNA_LOG_DIRS]
      logdna-agent-svc_rCURRENT.log    - agent windows service log
      winevt-tailer_tail1.log          - Windows Event Log Tailer log
  ```

### Chocolatey

The Chocolatey installation package is available at:

https://community.chocolatey.org/packages/mezmo-agent/

Assuming [Chocolatey Package Manager](https://chocolatey.org/install) is already installed run as Administrator:

```
choco install mezmo-agent [--install-arguments="KEY=<INGESTION_KEY>"]
```

To install specific version:

```
choco install mezmo-agent --version=<VERSION>
```
Get the version from Chocolatey web site at https://community.chocolatey.org/packages/mezmo-agent#versionhistory

### Advanced configuration

The tailer's effective configuration is always printed at the beginning of the log when agent starts. It can be modified and appended to Agent configuration file logdna.conf [yaml]. For advanced configuration options please refer to [Windows Event Log Tailer](https://github.com/logdna/winevt-tailer).
