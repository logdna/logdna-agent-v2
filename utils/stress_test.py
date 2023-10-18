import multiprocessing

from flask import Flask, request, Response, json
import time
import gzip
import io
import sys
import os
import threading
import multiprocessing as mp
import logging
from queue import Empty

class TestSeq:
    def __init__(self, test_seq, total_lines):
        self.test_seq = test_seq
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


def line_processor_loop(queue: mp.Queue, test_name: str):
    # request data processing loop
    test_sequences = {}
    log = logging.getLogger(test_name)
    while True:
        try:
            request_data = queue.get(block=True, timeout=15)
        except Empty:
            continue
        lines = None
        data_str = None
        try:
            data_str = request_data.decode("utf-8")
            data_json = json.loads(data_str)
            lines = data_json["lines"]
        except Exception as e:
            log.error(repr(e))
            log.error(f"line: '{data_str}'")
        for l in lines:
            try:
                line_obj = json.loads(l["line"])
                test_seq = line_obj.get("test_seq")
                if test_seq:
                    test_seq_obj = test_sequences.get(test_seq)
                    if not test_seq_obj:
                        test_seq_obj = TestSeq(test_seq, line_obj["total_lines"])
                        test_sequences[test_seq] = test_seq_obj
                    test_seq_obj.line_ids.append(line_obj["line_id"])
                    check_test_seq(test_seq_obj)
            except Exception as e:
                #            print(repr(e))
                pass


def check_test_seq(test: TestSeq) -> bool:
    if len(test.line_ids) >= test.total_lines:
        if (
            len(set(test.line_ids)) == test.total_lines
            and len(test.line_ids) == test.total_lines
        ):
            g_log.info(
                f"Test {test.test_seq}: received all {test.total_lines} lines. COMPLETED"
            )
            return True
        if len(set(test.line_ids)) <= test.total_lines:
            g_log.info(
                f"Test {test.test_seq}: received duplicate {test.total_lines - len(set(test.line_ids))} lines, "
                f"waiting ..."
            )
    return False


def log_writer_loop(test_seq, file_path, num_lines):
    # runs in separate process
    log = logging.getLogger(test_seq)
    log.setLevel(logging.INFO)
    log.info(f"open {file_path}")
    DELAY_S = 0.1
    try:
        os.remove(file_path)
        time.sleep(DELAY_S)
    except Exception as e:
        pass
    with open(file_path, "w") as f:
        log.info(f"start writer loop {file_path}")
        for i in range(1, num_lines + 1):
            line = {"test_seq": test_seq, "total_lines": num_lines, "line_id": i}
            f.write(f"{json.dumps(line)}\n")
            f.flush()
            time.sleep(DELAY_S)
    log.info(f"finished writer loop {file_path}")


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
    # start writers
    for i in range(1, num_files + 1):
        test_seq = f"{test_name}.{i:03d}"
        file_path = f"{log_dir}/{test_name}.{i:03d}.log"
        mp.Process(
            target=log_writer_loop,
            args=(
                test_seq,
                file_path,
                num_lines,
            ),
        ).start()
    # start line processing
    mp.Process(
        target=line_processor_loop,
        args=(g_queue, test_name),
    ).start()
    g_log.info("starting ingestor web server")
    app.run(host="0.0.0.0", port=7080, threaded=True)


if __name__ == "__main__":
    main()
