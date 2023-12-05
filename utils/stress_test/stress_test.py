from flask import Flask, request, json
import time
import gzip
import io
import sys
import os
import typing
import signal
import multiprocessing as mp
import logging
from queue import Empty
from timeit import default_timer as timer
import secrets
import argparse
from ratelimiter import RateLimiter
from environs import Env
import re
import time

REPORT_INTERVAL_S = 5

JSON_PATTERN = re.compile(r'^\s*(\{.*\}|\[.*\])\s*$')


class TestSeq:
    def __init__(self, seq_id, total_lines):
        self.seq_id = seq_id
        self.total_lines = total_lines
        self.line_ids = []


g_log = None
g_request_queue = mp.Queue(1000)
g_report_queue = mp.Queue()

logging.basicConfig(
    format="%(asctime)s %(levelname)-6s %(message)s",
    level=logging.INFO,
    datefmt="%Y-%m-%d %H:%M:%S",
)
logging.getLogger("werkzeug").setLevel(logging.ERROR)
app = Flask(__name__)


@app.route("/logs/status_308", methods=["GET", "POST"])
def request_status_308():
    global g_log
    print(request.__dict__)
    time.sleep(1)
    return "response text", 308


@app.route("/logs/status_500", methods=["GET", "POST"])
def request_status_500():
    global g_log
    print(request.__dict__)
    time.sleep(1)
    return "response text", 500


@app.route("/logs/agent", methods=["GET", "POST"])
def request_logs_agent():
    # print(request.__dict__)
    if request.headers.get("Content-Type") == "json":
        return "Expected: Encoding-Type=json", 415
    if request.headers.get("Content-Encoding") == "gzip":
        compressed_data = io.BytesIO(request.data)
        request_data = gzip.GzipFile(fileobj=compressed_data, mode="r").read()
    else:
        request_data = request.data
    g_request_queue.put(request_data, block=True)
    return "", 200


def request_processor_loop(
        request_queue: mp.Queue,
        report_queue: mp.Queue,
        test_id: str,
        num_files: int,
        main_pid: int,
):
    test_sequences = {}
    num_unrecognized_lines = 0
    received_line_bytes = 0
    writer_reports = {}
    start_ts = timer()
    last_report_ts = timer()
    first_line_ts = None
    log = logging.getLogger(test_id)
    while True:
        if last_report_ts + REPORT_INTERVAL_S < timer():
            last_report_ts = timer()
            # process reports from writers
            while not report_queue.empty():
                report = report_queue.get()
                writer_reports[report["seq_id"]] = report
            # check test status, report stats and stop test if completed
            if check_test_state(
                    test_id,
                    test_sequences,
                    num_files,
                    start_ts,
                    first_line_ts,
                    num_unrecognized_lines,
                    writer_reports,
                    received_line_bytes,
            ):
                log.info(f"FINISHED in {timer() - start_ts:.0f} sec")
                os.kill(main_pid, signal.SIGINT)
        try:
            request_data = request_queue.get(block=True, timeout=1)
        except Empty:
            continue
        data_str = None
        try:
            data_str = request_data.decode("utf-8")
            data_json = json.loads(data_str)
            lines = data_json["lines"]
        except Exception as e:
            log.error(repr(e))
            log.error(f"payload(1): '{data_str}'")
            continue
        for l in lines:
            if not first_line_ts:
                first_line_ts = timer()
            try:
                line_str = l["line"]
                received_line_bytes = received_line_bytes + len(line_str)
                # test line is json
                if not JSON_PATTERN.match(line_str):
                    num_unrecognized_lines = num_unrecognized_lines + 1
                    continue
                line_obj = json.loads(line_str)
                # test line has seq_id that starts with test_id
                seq_id: str = line_obj.get("seq_id")
                if not seq_id or not seq_id.startswith(test_id):
                    num_unrecognized_lines = num_unrecognized_lines + 1
                    continue
                test_seq_obj = test_sequences.get(seq_id)
                if not test_seq_obj:
                    test_seq_obj = TestSeq(seq_id, line_obj["total_lines"])
                    test_sequences[seq_id] = test_seq_obj
                test_seq_obj.line_ids.append(line_obj["line_id"])
            except Exception as e:
                log.warning(repr(e))
                log.warning(f"payload(2): '{data_str}'")
                num_unrecognized_lines = num_unrecognized_lines + 1
                pass


def check_test_state(
        test_id: str,
        test_sequences: typing.Dict[str, TestSeq],
        num_files: int,
        start_ts: float,
        first_line_ts: float,
        num_unrecognized_lines: int,
        writer_reports: dict,
        received_line_bytes: int,
) -> bool:
    num_total_seq = num_files
    num_received_seq = 0
    num_completed_seq = 0
    num_total_lines = 0
    num_received_lines = 0
    num_duplicate_lines = 0
    num_committed_lines = 0
    run_time = timer() - start_ts
    last_commit_ts = 0.0
    for report in writer_reports.values():
        num_committed_lines = num_committed_lines + report["num_committed_lines"]
        last_commit_ts = max(last_commit_ts, report["timestamp"])
    for seq in test_sequences.values():
        num_received_seq = num_received_seq + 1
        num_total_lines = num_total_lines + seq.total_lines
        num_received_lines = num_received_lines + len(seq.line_ids)
        if len(seq.line_ids) >= seq.total_lines:
            if (
                    len(set(seq.line_ids)) == seq.total_lines
                    and len(seq.line_ids) == seq.total_lines
            ):
                num_completed_seq = num_completed_seq + 1
                continue
            if len(set(seq.line_ids)) <= seq.total_lines:
                num_duplicate_lines = (
                        num_duplicate_lines + seq.total_lines - len(set(seq.line_ids))
                )
    if first_line_ts:
        received_line_rate = num_received_lines / (timer() - first_line_ts)
    else:
        received_line_rate = 0
    committed_line_rate = num_committed_lines / (last_commit_ts - start_ts)
    g_log.info(f"")
    g_log.info(f"total seq:           {num_total_seq}")
    g_log.info(f"received seq:        {num_received_seq}")
    g_log.info(f"completed seq:       {num_completed_seq}")
    g_log.info(f"total lines:         {num_total_lines}")
    g_log.info(f"committed lines:     {num_committed_lines}")
    g_log.info(f"received lines:      {num_received_lines}")
    g_log.info(f"duplicate lines:     {num_duplicate_lines}")
    g_log.info(f"unrecognized lines:  {num_unrecognized_lines}")
    g_log.info(f"committed line rate: {committed_line_rate:.0f} per sec")
    g_log.info(f"received line rate:  {received_line_rate:.0f} per sec")
    g_log.info(f"received line bytes: {received_line_bytes / 1000:.0f} KB")
    g_log.info(f"test id:             {test_id}")
    g_log.info(f"run time:            {run_time:.0f} sec")
    return num_total_seq == num_completed_seq


def log_writer_loop(
        seq_id: str,
        file_path: str,
        report_queue: mp.Queue,
        num_lines: int,
        override: bool,
        file_line_rate: int,
):
    # runs in separate process
    log = logging.getLogger(seq_id)
    log.setLevel(logging.INFO)
    log.debug(f"open {file_path}")
    if override:
        try:
            os.remove(file_path)
            time.sleep(0.1)
        except Exception:
            pass
    rate_limiter = RateLimiter(max_calls=file_line_rate, period=1)
    last_report_ts = timer()
    with open(file_path, "a") as f:
        log.debug(f"start writer loop {file_path}")
        for i in range(1, num_lines + 1):
            with rate_limiter:
                line = {"seq_id": seq_id, "total_lines": num_lines, "line_id": i}
                f.write(f"{json.dumps(line)}\n")
                f.flush()
            if timer() - last_report_ts >= REPORT_INTERVAL_S / 2:
                report = {
                    "seq_id": seq_id,
                    "num_committed_lines": i,
                    "timestamp": timer(),
                }
                report_queue.put(report)
                last_report_ts = timer()
    # final report
    report = {
        "seq_id": seq_id,
        "num_committed_lines": i,
        "timestamp": timer(),
    }
    report_queue.put(report)
    log.debug(f"finished writer loop {file_path}")


def assert_log_dir(log_dir: str) -> str:
    assert os.path.exists(log_dir), f"Directory does not exist: {log_dir}"
    assert os.path.isdir(log_dir), f"Path is not a directory: {log_dir}"
    assert os.access(log_dir, os.W_OK), f"Directory is not writable: {log_dir}"
    return log_dir


def assert_positive_integer(value_str: str) -> int:
    value_int = int(value_str)
    assert value_int > 0, f"Value is not a positive integer: {value_str}"
    return value_int


def assert_non_negative_integer(value_str: str) -> int:
    value_int = int(value_str)
    assert value_int >= 0, f"Value is not non-negative integer: {value_str}"
    return value_int


def main():
    global g_log
    # get env vars
    env = Env()
    if len(sys.argv) == 1:
        positional_args = [os.environ.get('ST_LOG_DIR')]
        if all(arg is not None for arg in positional_args):
            sys.argv.extend(positional_args)

    # create arg parser
    parser = argparse.ArgumentParser(
        description="Agent Stress Test",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "log_dir",
        type=assert_log_dir,
        help="Directory for test log files. [ST_LOG_DIR]",
    )
    parser.add_argument(
        "--files",
        type=assert_positive_integer,
        help="Number of log files. [ST_NUM_LOG_FILES]",
        dest="num_log_files",
        default=env.int("ST_NUM_LOG_FILES", 10),
    )
    parser.add_argument(
        "--lines",
        type=assert_positive_integer,
        help="Number of lines to add to each log file. [ST_LINES_PER_FILE]",
        dest="num_lines",
        default=env.int("ST_LINES_PER_FILE", 1000),
    )
    parser.add_argument(
        "--line-rate",
        type=assert_positive_integer,
        help="Line rate (per second) per each file. [ST_FILE_LINE_RATE]",
        dest="file_line_rate",
        default=env.int("ST_FILE_LINE_RATE", 50),
    )
    parser.add_argument(
        "--port",
        type=assert_positive_integer,
        help="Ingestor web server port. [ST_PORT]",
        dest="port",
        default=env.int("ST_PORT", 7080),
    )
    parser.add_argument(
        "--override",
        help="Override existing log files. [ST_OVERRIDE]",
        dest="override",
        action="store_true",
        default=env.bool("ST_OVERRIDE", False),
    )
    parser.add_argument(
        "--startup-delay",
        type=assert_non_negative_integer,
        help="Number of seconds to delay the startup. [ST_STARTUP_DELAY]",
        dest="startup_delay",
        default=env.int("ST_STARTUP_DELAY", 0),
    )
    args = parser.parse_args()
    #
    app_name = os.path.splitext(os.path.basename(sys.argv[0]))[0]
    test_id = f"{app_name}-{secrets.token_hex(3)}"
    g_log = logging.getLogger(test_id)
    g_log.setLevel(logging.INFO)
    # print config
    for arg in vars(args):
        g_log.info(f'{arg}: {getattr(args, arg)}')
    # delay startup
    if args.startup_delay:
        g_log.info(f'delaying startup for {args.startup_delay} seconds ...')
        time.sleep(args.startup_delay)
    # start log writers
    for i in range(1, args.num_log_files + 1):
        seq_id = f"{test_id}-{i}"
        file_path = f"{args.log_dir}/{app_name}.{i:03d}.log"
        mp.Process(
            target=log_writer_loop,
            args=(
                seq_id,
                file_path,
                g_report_queue,
                args.num_lines,
                args.override,
                args.file_line_rate,
            ),
            daemon=True,
        ).start()
    # start request processing
    mp.Process(
        target=request_processor_loop,
        args=(
            g_request_queue,
            g_report_queue,
            test_id,
            args.num_log_files,
            os.getpid(),
        ),
        daemon=True,
    ).start()
    g_log.info("starting ingestor web server")
    app.run(host="0.0.0.0", port=args.port, threaded=True)


if __name__ == "__main__":
    main()
