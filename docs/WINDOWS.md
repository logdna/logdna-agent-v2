# Mezmo Agent for Windows

## Installation

- Installation directories and files:

  ```
  C:\Program Files\Mezmo               - application files
  C:\ProgramData\logdna\logdna.conf    - Agent configuration file
  C:\ProgramData\logdna\agent_state.db - location of log offsets database [LOGDNA_DB_PATH]
  C:\ProgramData\logs                  - log files directory monitored by Agent [LOGDNA_LOG_DIRS]
  ```

### MSI

- Package location

  https://logdna-agent-build-bin.s3.amazonaws.com/3.8.6/x86_64-pc-windows-msvc/signed/mezmo-agent.msi

- Installation parameters

  * Ingestion Key can be specfiied in MSI command line:
    ```
    mezmo_agent.msi KEY="<YOUR INGESTIOn KEY>"
    ```

  * The key can also be specified in config file:
    ```
    c:\ProgramData\logdna\logdna.conf

    http:
      ingestion_key: <YOUR_INGESTION_KEY>
    ```

  * Other Installation parameters:
    ```
    Display Options
      /quiet
        Quiet mode, no user interaction
      /passive
        Unattended mode - progress bar only
      /q[n|b|r|f]
        Sets user interface level
        n - No UI
        b - Basic UI
        r - Reduced UI
        f - Full UI (default)
      /help
        Help information

    Restart Options
      /norestart
        Do not restart after the installation is complete
      /promptrestart
        Prompts the user for restart if necessary
      /forcerestart
        Always restart the computer after installation

    Logging Options
      /l[i|w|e|a|r|u|c|m|o|p|v|x|+|!|*] <LogFile>
        i - Status messages
        w - Nonfatal warnings
        e - All error messages
        a - Start up of actions
        r - Action-specific records
        u - User requests
        c - Initial UI parameters
        m - Out-of-memory or fatal exit information
        o - Out-of-disk-space messages
        p - Terminal properties
        v - Verbose output
        x - Extra debugging information
        + - Append to existing log file
        ! - Flush each line to the log
        * - Log all information, except for v and x options
      /log <LogFile>
        Equivalent of /l* <LogFile>

    Setting Public Properties
      [KEY=<YOUR_INGESTION_KEY>]
    ```

- Unattended installation example:
    ```
    mezmo_agent.msi /qn KEY=abcdefg1234567890
    ```

### Chocolatey
- Package location

https://community.chocolatey.org/packages/mezmo-agent/

- Installation

Run:

```
choco install mezmo-agent
```
