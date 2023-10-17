from flask import Flask, request, Response, json
import time
import gzip
import io
import sys
import os
import threading
from multiprocessing import Process
import logging


class Test:
    def __init__(self, test_name, total_lines):
        self.test_name = test_name
        self.total_lines = total_lines
        self.line_ids = []


g_log = None
g_tests = {}
logging.basicConfig(
    format="%(asctime)s %(levelname)-6s %(message)s",
    level=logging.INFO,
    datefmt="%Y-%m-%d %H:%M:%S",
)
logging.getLogger("werkzeug").setLevel(logging.ERROR)
app = Flask(__name__)


@app.route("/logs/status_308", methods=["GET", "POST"])
def status_308():
    global g_log
    print(request.__dict__)
    time.sleep(1)
    return "response text", 308


@app.route("/logs/agent", methods=["GET", "POST"])
def index():
    # print(request.__dict__)
    if request.headers.get("Content-Type") == "json":
        return "Expected: Encoding-Type=json", 415
    data = request.data
    if request.headers.get("Content-Encoding") == "gzip":
        compressed_data = io.BytesIO(request.data)
        data = gzip.GzipFile(fileobj=compressed_data, mode="r").read()
    data_json = json.loads(data.decode("utf-8"))
    lines = data_json["lines"]
    for l in lines:
        try:
            line_obj = json.loads(l["line"])
            test_name = line_obj.get("test_name")
            if test_name:
                test_obj = g_tests.get(test_name)
                if not test_obj:
                    test_obj = Test(test_name, line_obj["total_lines"])
                    g_tests[test_name] = test_obj
                test_obj.line_ids.append(line_obj["line_id"])
                check_test(test_obj)
        except Exception as e:
            #            print(repr(e))
            pass
    return "", 200


def check_test(test: Test) -> bool:
    if len(test.line_ids) >= test.total_lines:
        if (
            len(set(test.line_ids)) == test.total_lines
            and len(test.line_ids) == test.total_lines
        ):
            g_log.info(
                f"Test {test.test_name}: received all {test.total_lines} lines. COMPLETED"
            )
            return True
        if len(set(test.line_ids)) <= test.total_lines:
            g_log.info(
                f"Test {test.test_name}: received duplicate {test.total_lines - len(set(test.line_ids))} lines, "
                f"waiting ..."
            )
    return False


def log_write_loop(test_name, file_name, num_lines):
    # runs in separate process
    log = logging.getLogger(test_name)
    log.setLevel(logging.INFO)
    log.info(f"open {file_name}")
    DELAY_S = 0.1
    try:
        os.remove(file_name)
        time.sleep(DELAY_S)
    except Exception as e:
        pass
    file1 = open(file_name, "w")
    log.info(f"start write loop {file_name}")
    for i in range(1, num_lines + 1):
        line = {"test_name": test_name, "total_lines": num_lines, "line_id": i}
        file1.write(f"{json.dumps(line)}\n")
        file1.flush()
        time.sleep(DELAY_S)
    log.info(f"finished write loop {file_name}")


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
    for i in range(1, num_files + 1):
        Process(
            target=log_write_loop,
            args=(f"{test_name}.{i:03d}", f"{log_dir}/{test_name}.{i:03d}.log", num_lines),
        ).start()
    g_log.info("starting web server")
    app.run(host="0.0.0.0", port=7080)


if __name__ == "__main__":
    main()
