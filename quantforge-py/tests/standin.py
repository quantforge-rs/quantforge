"""What the client's tests share: a server of the test's own, on this
machine, which answers what a test tells it to and records what it was
asked, so that each test says exactly what a server said
(docs/python-client.md, docs/python-bundles.md).

No QuantForge server refuses a token, answers out of turn or sends what the
protocol does not write, and a client has to be held to each: a stand-in
does what a test asks of it. What the client does with a real `serve run`
is in the tests that start one (`jobs.py`).
"""

import hashlib
import http.server
import json
import threading

import quantforge_py as qf

# What a server of this release says of itself.
SPEAKS = {"version": "0.2.0", "proto_version": 1, "min_supported": 1}

# One job, as the protocol writes its status.
JOB = "57917a9825f44d75ad4a2e6329d00ec7"
BUNDLE = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"


def written(job_id=JOB, state=None, **changed) -> dict:
    """A job's status as a server writes it: queued, unless `state` says
    otherwise, with the members `changed` names in the place of its own."""
    status = {
        "job_id": job_id,
        "submitted_at_ms": 1_792_454_400_000,
        "submission": {
            "bundle": BUNDLE,
            "data": {
                "exchange": "binance",
                "category": "spot",
                "symbol": "BTCUSDT",
                "interval": "1m",
                "start_open_ms": 1_792_454_400_000,
                "end_open_ms": 1_792_540_740_000,
            },
            "backtest": {"initial_cash": "10000", "fee_bps": "7.50", "close_out_at_end": True},
        },
        "state": state or {"kind": "queued"},
    }
    status.update(changed)
    return status


def refusal(status: int, code: str, message: str, headers=None, **more):
    """An answer that is the protocol's error envelope."""
    return status, {"error": {"code": code, "message": message, **more}}, headers or {}


class StandIn:
    """A server of the test's own, on this machine: it answers each route
    with what the test told it to, one answer after another and the last
    one from then on, `GET /version` with `SPEAKS` unless told otherwise,
    and any other route as `answering` makes of it: as a QuantForge server
    answers one it does not have, unless a stand-in derived from this one
    says otherwise. An answer that is a function is called as it is asked
    for, which is how a test holds one back. `asked` is what it was asked,
    in order."""

    def __init__(self, speaks=SPEAKS):
        self.asked = []
        self._answers = {("GET", "/version"): [(200, speaks, {})]}
        stand_in = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def answer(self):
                length = int(self.headers.get("Content-Length") or 0)
                sent = self.rfile.read(length) if length else b""
                stand_in.asked.append((self.command, self.path, dict(self.headers), sent))
                answers = stand_in._answers.get((self.command, self.path))
                if not answers:
                    made = stand_in.answering(self.command, self.path, sent)
                    answers = None if made is None else [made]
                if not answers:
                    answers = [
                        refusal(
                            404, "not_found", f"this server answers no {self.command} {self.path}"
                        )
                    ]
                answer = answers.pop(0) if len(answers) > 1 else answers[0]
                if callable(answer):
                    answer = answer()
                status, body, headers = answer
                if isinstance(body, (dict, list)):
                    body = json.dumps(body).encode()
                    headers = {"Content-Type": "application/json", **headers}
                elif isinstance(body, str):
                    body = body.encode()
                self.send_response(status)
                for name, value in headers.items():
                    self.send_header(name, value)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            do_GET = do_POST = do_PUT = answer

            def log_message(self, *arguments):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True
        )
        self.thread.start()

    def answering(self, method: str, target: str, sent: bytes):
        """What a route the test told it nothing of is answered with, from
        what was sent: a status, a body and headers, or `None` for the
        refusal of a route it does not have. A stand-in derived from this
        one answers routes whose path or answer a test cannot write ahead."""
        return None

    @property
    def port(self) -> int:
        return self.server.server_address[1]

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def says(self, method: str, target: str, *answers) -> "StandIn":
        """From now on `method target` is answered with `answers`, each a
        status, a body and headers, in order."""
        self._answers[(method, target)] = [
            answer if callable(answer) or len(answer) == 3 else (*answer, {}) for answer in answers
        ]
        return self

    @property
    def routes(self) -> list[str]:
        """What it was asked, each as its method and its target."""
        return [f"{method} {target}" for method, target, *_ in self.asked]

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.close()


# What a stand-in that runs bundles offers and stores: the Python its
# workers run, and an hour of one-minute bars from noon on the first day of
# 2026, UTC.
PYTHON = "3.12"
FIRST = 1_767_268_800_000
MINUTE = 60_000
LAST = FIRST + 59 * MINUTE


def stored(first=FIRST, last=LAST, **named) -> dict:
    """A market as a server lists it."""
    market = {"exchange": "binance", "category": "spot", "symbol": "BTCUSDT", "interval": "1m"}
    return {**market, **named, "first_open_ms": first, "last_open_ms": last}


class Serving(StandIn):
    """A stand-in that runs bundles, as far as a client can tell: it offers
    a runtime, lists the market, stores an upload under the hash of its
    bytes and answers its receipt, and queues a submission as a job of
    what was submitted."""

    def __init__(self, python=PYTHON, packages=None, markets=None, engine=None):
        super().__init__()
        runtime = {
            "engine": engine or qf.ENGINE_VERSION,
            "python": python,
            "packages": packages if packages is not None else {"numpy": ["2.4.6"]},
        }
        self.says("GET", "/v1/runtime", (200, {"runtime": runtime}))
        listed = [stored()] if markets is None else markets
        self.says("GET", "/v1/markets", (200, {"markets": listed}))
        self.held = {}

    def answering(self, method, target, sent):
        if method == "PUT" and target.startswith("/v1/bundles/"):
            named = target.removeprefix("/v1/bundles/")
            assert named == "sha256:" + hashlib.sha256(sent).hexdigest()
            status = 200 if named in self.held else 201
            self.held[named] = sent
            return status, {"code_hash": named, "size_bytes": len(sent)}, {}
        if method == "POST" and target == "/v1/backtests":
            return 201, written(submission=json.loads(sent)), {"Location": f"/v1/backtests/{JOB}"}
        return None

    def sent(self, method) -> list[tuple[str, dict, bytes]]:
        """Every request of a method: its target, its headers and its body."""
        return [
            (target, headers, body)
            for asked, target, headers, body in self.asked
            if asked == method
        ]
