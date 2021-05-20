# LogDNA Agent Internal Metrics

The LogDNA agent records metrics that can be relevant for monitoring and alerting, such as number log files currently
tracked or number of bytes parsed, along with process status information.

## Exposing the Agent Metrics as a Prometheus Endpoint 

You can enable the agent's [Prometheus][prometheus] endpoint to expose the internal metrics to a Prometheus server,
by setting the `LOGDNA_METRICS_PORT` environment variable to an available port number, for example:

```yaml
    - env:
        - name: LOGDNA_METRICS_PORT
          value: "9881"
```

Metrics related to the agent are exposed using the prefix `logdna_agent_` and process status information, like memory
and cpu usage, are exposed with the prefix `process_`.

## Enabling Prometheus target discovery on Kubernetes

Prometheus implements service discovery within Kubernetes, automatically scraping Kubernetes resources that
define the annotations. You only need to specify the annotations in the agent DaemonSet metadata template for
Prometheus to scrape the metrics from each pod.

```yaml
apiVersion: apps/v1
kind: DaemonSet
spec:
  template:
    metadata:
      annotations:
        prometheus.io/port: "9881"
        prometheus.io/scrape: "true"
  # ... rest of daemonset settings ...
```

After applying the DaemonSet with the Prometheus annotations, the metrics will be available in your Prometheus server.

## Metrics in log messages

The agent also publishes its internal metrics every minute as a log line. This was useful in older versions for
observability, but it's now deprecated in favor of Prometheus support.

If you want to continue using these log line metrics, you should consider that there has been the following changes in
version 3.3 and above of the agent:

- Counters such as `"fs.events"`, `"ingest.requests"` and `"ingest.requests_size"`, etc are now monotonically
increasing counters as opposed to counters that got reset every minute.
- There are new metrics like `"ingest.requests_duration"` and `"fs.files_tracked"`.

[prometheus]: https://prometheus.io/
