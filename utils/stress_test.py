import multiprocessing

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


class TestSeq:
    def __init__(self, seq_name, total_lines):
        self.seq_name = seq_name
        self.total_lines = total_lines
        self.line_ids = []


g_log = None
g_queue = multiprocessing.Queue(100)

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
    g_queue.put(request_data, block=True)
    return "", 200


def request_processor_loop(queue: mp.Queue, test_name: str, num_files, main_pid: int):
    # request data processing loop
    test_sequences = {}
    unrecognized_lines = 0
    start_ts = timer()
    last_report_ts = timer()
    log = logging.getLogger(test_name)
    while True:
        if last_report_ts + 5 < timer():
            last_report_ts = timer()
            if check_test_state(
                test_sequences, num_files, start_ts, unrecognized_lines
            ):
                log.info(f"FINISHED in {timer() - start_ts:.0f} sec")
                os.kill(main_pid, signal.SIGINT)
        try:
            request_data = queue.get(block=True, timeout=1)
        except Empty:
            continue
        data_str = None
        try:
            data_str = request_data.decode("utf-8")
            data_json = json.loads(data_str)
            lines = data_json["lines"]
        except Exception as e:
            log.error(repr(e))
            log.error(f"line: '{data_str}'")
            continue
        for l in lines:
            try:
                line_obj = json.loads(l["line"])
                seq_name: str = line_obj.get("seq_name")
                if seq_name and seq_name.startswith(test_name):
                    test_seq_obj = test_sequences.get(seq_name)
                    if not test_seq_obj:
                        test_seq_obj = TestSeq(seq_name, line_obj["total_lines"])
                        test_sequences[seq_name] = test_seq_obj
                    test_seq_obj.line_ids.append(line_obj["line_id"])
                else:
                    unrecognized_lines = unrecognized_lines + 1
            except Exception as e:
                log.error(repr(e))
                pass


def check_test_state(
    test_sequences: typing.Dict[str, TestSeq],
    num_files: int,
    start_ts: float,
    unrecognized_lines: int,
) -> bool:
    num_total_seq = num_files
    num_received_seq = 0
    num_completed_seq = 0
    num_total_lines = 0
    num_received_lines = 0
    num_duplicate_lines = 0
    run_time = timer() - start_ts
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
    line_rate = num_total_lines / run_time
    g_log.info(f"")
    g_log.info(f"total seq:           {num_total_seq}")
    g_log.info(f"received seq:        {num_received_seq}")
    g_log.info(f"completed seq:       {num_completed_seq}")
    g_log.info(f"total lines:         {num_total_lines}")
    g_log.info(f"received lines:      {num_received_lines}")
    g_log.info(f"duplicate lines:     {num_duplicate_lines}")
    g_log.info(f"unrecognized lines:  {unrecognized_lines}")
    g_log.info(f"line rate:           {line_rate:.0f} per sec")
    g_log.info(f"run time:            {run_time:.0f} sec")
    return num_total_seq == num_completed_seq


def log_writer_loop(seq_name, file_path, num_lines):
    # runs in separate process
    log = logging.getLogger(seq_name)
    log.setLevel(logging.INFO)
    log.debug(f"open {file_path}")
    DELAY_S = 0.1
    try:
        # os.remove(file_path)
        time.sleep(DELAY_S)
    except Exception:
        pass
    with open(file_path, "a") as f:
        log.debug(f"start writer loop {file_path}")
        for i in range(1, num_lines + 1):
            line = {"seq_name": seq_name, "total_lines": num_lines, "line_id": i}
            f.write(f"{json.dumps(line)}\n")
            f.flush()
            time.sleep(DELAY_S)
    log.debug(f"finished writer loop {file_path}")


def main():
    global g_log
    if len(sys.argv) < 4:
        print(
            f"\npython {sys.argv[0]}  <log dir>  <number of log files>  <number of lines>\n"
        )
        quit()
    test_name = os.path.splitext(os.path.basename(sys.argv[0]))[0]
    g_log = logging.getLogger(test_name)
    g_log.setLevel(logging.INFO)
    log_dir = sys.argv[1]
    num_files = int(sys.argv[2])
    num_lines = int(sys.argv[3])
    # start log writers
    for i in range(1, num_files + 1):
        seq_name = f"{test_name}-{secrets.token_hex(4)}"
        file_path = f"{log_dir}/{test_name}.{i:03d}.log"
        mp.Process(
            target=log_writer_loop,
            args=(
                seq_name,
                file_path,
                num_lines,
            ),
            daemon=True,
        ).start()
    # start request processing
    mp.Process(
        target=request_processor_loop,
        args=(g_queue, test_name, num_files, os.getpid()),
        daemon=True,
    ).start()
    g_log.info("starting ingestor web server")
    app.run(host="0.0.0.0", port=7080, threaded=True)


if __name__ == "__main__":
    main()
