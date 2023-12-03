# Agent Stress Test:

- produces configurable number of log files and lines (limited by ulimit)
- acts as ingester endpoint for agent (0.0.0.0:7080)
- checks for duplicate and lost lines
- prints periodic reports until all lines received from agent
- supports all agent versions
- supports gzip compression
- does not override existing log files, appends by default
- Todo: add https support

<!-- TOC -->

* [Stress test:](#stress-test)
    * [Quick start](#quick-start)
        * [Start Agent with following env vars:](#start-agent-with-following-env-vars)
        * [Start Test](#start-test)
    * [Running in Docker](#running-in-docker)
        * [Build Test image](#build-test-image)
        * [Run Test and Agent Docker images](#run-test-and-agent-docker-images)
    * [Running in K8s](#running-in-k8s)
    * [Testing strategies](#testing-strategies)
    * [Help](#help)

<!-- TOC -->

## Quick start

Test runs together with dedicated agent instance.

### Start Agent with following env vars:

```bash
export MZ_LOG_DIRS=/tmp/stress_test
export MZ_HOST=localhost:7080
export MZ_USE_SSL=false
```

### Start Test

Supplied script [utils/stress_test/run](./run) runs short low log volume test:

```bash
$ python utils/stress_test/run
2023-12-03 20:08:28 INFO   log_dir: /tmp/stress_test
2023-12-03 20:08:28 INFO   num_log_files: 10
2023-12-03 20:08:28 INFO   num_lines: 1000
2023-12-03 20:08:28 INFO   file_line_rate: 50
2023-12-03 20:08:28 INFO   port: 7080
2023-12-03 20:08:28 INFO   override: False
2023-12-03 20:08:28 INFO   starting ingestor web server
 * Serving Flask app 'stress_test'
 * Debug mode: off
2023-12-03 20:08:33 INFO
2023-12-03 20:08:33 INFO   total seq:           10
2023-12-03 20:08:33 INFO   received seq:        10
2023-12-03 20:08:33 INFO   completed seq:       0
2023-12-03 20:08:33 INFO   total lines:         10000
2023-12-03 20:08:33 INFO   committed lines:     1510
2023-12-03 20:08:33 INFO   received lines:      3000
2023-12-03 20:08:33 INFO   duplicate lines:     0
2023-12-03 20:08:33 INFO   unrecognized lines:  0
2023-12-03 20:08:33 INFO   committed line rate: 503 per sec
2023-12-03 20:08:33 INFO   received line rate:  597 per sec
2023-12-03 20:08:33 INFO   received line bytes: 212 KB
2023-12-03 20:08:33 INFO   test id:             stress_test-a028f7
2023-12-03 20:08:33 INFO   run time:            5 sec
2023-12-03 20:08:38 INFO
2023-12-03 20:08:38 INFO   total seq:           10
2023-12-03 20:08:38 INFO   received seq:        10
2023-12-03 20:08:38 INFO   completed seq:       0
2023-12-03 20:08:38 INFO   total lines:         10000
2023-12-03 20:08:38 INFO   committed lines:     4510
2023-12-03 20:08:38 INFO   received lines:      5000
2023-12-03 20:08:38 INFO   duplicate lines:     0
2023-12-03 20:08:38 INFO   unrecognized lines:  0
2023-12-03 20:08:38 INFO   committed line rate: 501 per sec
2023-12-03 20:08:38 INFO   received line rate:  498 per sec
2023-12-03 20:08:38 INFO   received line bytes: 354 KB
2023-12-03 20:08:38 INFO   test id:             stress_test-a028f7
2023-12-03 20:08:38 INFO   run time:            10 sec
2023-12-03 20:08:43 INFO
2023-12-03 20:08:43 INFO   total seq:           10
2023-12-03 20:08:43 INFO   received seq:        10
2023-12-03 20:08:43 INFO   completed seq:       0
2023-12-03 20:08:43 INFO   total lines:         10000
2023-12-03 20:08:43 INFO   committed lines:     7510
2023-12-03 20:08:43 INFO   received lines:      7500
2023-12-03 20:08:43 INFO   duplicate lines:     0
2023-12-03 20:08:43 INFO   unrecognized lines:  0
2023-12-03 20:08:43 INFO   committed line rate: 500 per sec
2023-12-03 20:08:43 INFO   received line rate:  498 per sec
2023-12-03 20:08:43 INFO   received line bytes: 532 KB
2023-12-03 20:08:43 INFO   test id:             stress_test-a028f7
2023-12-03 20:08:43 INFO   run time:            15 sec
2023-12-03 20:08:49 INFO
2023-12-03 20:08:49 INFO   total seq:           10
2023-12-03 20:08:49 INFO   received seq:        10
2023-12-03 20:08:49 INFO   completed seq:       10
2023-12-03 20:08:49 INFO   total lines:         10000
2023-12-03 20:08:49 INFO   committed lines:     10000
2023-12-03 20:08:49 INFO   received lines:      10000
2023-12-03 20:08:49 INFO   duplicate lines:     0
2023-12-03 20:08:49 INFO   unrecognized lines:  0
2023-12-03 20:08:49 INFO   committed line rate: 526 per sec
2023-12-03 20:08:49 INFO   received line rate:  498 per sec
2023-12-03 20:08:49 INFO   received line bytes: 710 KB
2023-12-03 20:08:49 INFO   test id:             stress_test-a028f7
2023-12-03 20:08:49 INFO   run time:            20 sec
2023-12-03 20:08:49 INFO   FINISHED in 20 sec
```

## Running in Docker

### Build Test image

```bash
cd utils/stress-test
docker build -t logdna-agent-stress-test .
```

### Run Test and Agent Docker images

NOTE: Make sure log dir on host, that is mapped into container, exists. Otherwise, it will be created under root user
automatically and will not be writable.

```bash
# create network
docker network create stress_test

# create folder for log file
mkdir /tmp/stress_test

# start stress test using official Docker image published on Docker.io
docker run --rm --net stress_test --name stress-test -u $(id -u):$(id -g) -v /tmp/stress_test:/tmp/stress_test logdna/logdna-agent-stress-test:3.9.0-dev /tmp/stress_test

# start agent against stress test ingestor
docker run -it --net stress_test -e MZ_INGESTION_KEY=blah -e MZ_LOG_DIRS=/var/log -e MZ_HOST=stress-test:7080 -e MZ_USE_SSL=false -it -v $(pwd)/logs:/var/log logdna/logdna-agent:3.9.0-dev
```

## Running in K8s

Test can run as a separate container in a pod that is part Daemon Set, that is based agent DS.
One test pod includes 2 containers:

- stress-test
- logdna-agent

Containers share one volume that is used as test log directory.
See yaml file: [k8s/agent-stress-test.yaml](../../k8s/agent-stress-test.yaml)

```bash
cd k8s
kubectl apply -f agent-namespace.yaml
kubectl apply -f agent-stress-test.yaml
```

## Testing strategies

The test can be used to:

- simulate specific log volume and line rate for an issue reproduction (on customer side). Knobs:
    - number of log files, env var ST_NUM_LOG_FILES
    - line rate per file, env var ST_FILE_LINE_RATE
- find proper cpu and memory resource limits in particular system setup

## Help

```bash
$ python utils/stress_test/stress_test.py -h
usage: stress_test.py [-h] [--files NUM_LOG_FILES] [--lines NUM_LINES]
                      [--line-rate FILE_LINE_RATE] [--port PORT] [--override]
                      log_dir

Agent Stress Test

positional arguments:
  log_dir               Directory for test log files. [ST_LOG_DIR]

optional arguments:
  -h, --help            show this help message and exit
  --files NUM_LOG_FILES
                        Number of log files. [ST_NUM_LOG_FILES] (default: 50)
  --lines NUM_LINES     Number of lines to add to each log file.
                        [ST_LINES_PER_FILE] (default: 10000)
  --line-rate FILE_LINE_RATE
                        Line rate (per second) per each file.
                        [ST_FILE_LINE_RATE] (default: 1000)
  --port PORT           Ingestor web server port. [ST_PORT] (default: 7080)
  --override            Override existing log files. [ST_OVERRIDE] (default:
                        False)
                        (default: False)
```
