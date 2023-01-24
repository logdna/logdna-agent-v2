# Mezmo Agent for Windows

## Installation

### Chocolatey
- Package location

https://community.chocolatey.org/packages/mezmo-agent/

- Installation

Run:

```
choco install mezmo-agent
```

### Installation directories and files:

  ```
  C:\Program Files\Mezmo               - application files
  C:\ProgramData\logdna\logdna.conf    - Agent configuration file
  C:\ProgramData\logdna\agent_state.db - location of log offsets database [LOGDNA_DB_PATH]
  C:\ProgramData\logs                  - log files directory monitored by Agent [LOGDNA_LOG_DIRS]
 ```