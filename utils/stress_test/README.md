Stress test:
- produces configurable number of log files and lines (limited by ulimit)
- acts as ingester endpoint for agent
- checks for duplicate and lost lines
- prints periodic reports until all lines received from agent
- supports all agent versions
- supports gzip compression
- does not override existing log files, appends by default
- todo: add https support

1. Running from shell

1.1 start agent with env vars:
```
export LOGDNA_LOG_DIRS=/home/dmitri/SOURCE/TMP/root/test
export LOGDNA_HOST=localhost:7080
export LOGDNA_USE_SSL=false
```

1.2 start test
```
$ python utils/stress_test/stress_test.py /home/dmitri/SOURCE/TMP/root/test 10 5000
2023-10-19 22:35:27 INFO   starting ingestor web server
 * Serving Flask app 'stress_test'
 * Debug mode: off
2023-10-19 22:35:33 INFO
2023-10-19 22:35:33 INFO   total seq:           10
2023-10-19 22:35:33 INFO   received seq:        10
2023-10-19 22:35:33 INFO   completed seq:       10
2023-10-19 22:35:33 INFO   total lines:         50000
2023-10-19 22:35:33 INFO   committed lines:     50000
2023-10-19 22:35:33 INFO   received lines:      50000
2023-10-19 22:35:33 INFO   duplicate lines:     0
2023-10-19 22:35:33 INFO   unrecognized lines:  0
2023-10-19 22:35:33 INFO   committed line rate: 8341 per sec
2023-10-19 22:35:33 INFO   received line rate:  8643 per sec
2023-10-19 22:35:33 INFO   received line bytes: 3939 KB
2023-10-19 22:35:33 INFO   run time:            6 sec
2023-10-19 22:35:33 INFO   FINISHED in 6 sec
```

2. Running in Docker

2.1 build test image
```
cd utils/stress-test
docker build -t  logdna-agent-stress-test
```

2.2 run test and agent images
```
docker network create stress_test
docker run --rm --net stress_test --name stress-test -v $(pwd)/logs:/var/log logdna-agent-stress-test:latest /var/log 50 1000000
docker run -it --net stress_test -e MZ_INGESTION_KEY=blah -e LOGDNA_LOG_DIRS=/var/log -e LOGDNA_HOST=stress-test:7080 -e LOGDNA_USE_SSL=false -it -v $(pwd)/logs:/var/log logdna/logdna-agent:3.9.0-dev
```

4. Help
```
$ python utils/stress_test/stress_test.py -h
usage: stress_test.py [-h] [--line_rate LINE_RATE] [--override] log_dir num_log_files num_lines

Agent Stress Test

positional arguments:
  log_dir               Directory where log files are stored.
  num_log_files         Number of log files to use.
  num_lines             Number of lines to add to each log file.

optional arguments:
  -h, --help            show this help message and exit
  --line_rate LINE_RATE
                        Line rate per log file. (default: 100)
  --override            Override existing log files. (default: False)
```
