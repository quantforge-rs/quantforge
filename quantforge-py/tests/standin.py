"""What the client's tests share: a server of the test's own, on this
machine, which answers what a test tells it to and records what it was
asked, so that each test says exactly what a server said
(docs/python-client.md, docs/python-bundles.md).

No QuantForge server refuses a token, answers out of turn or sends what the
protocol does not write, and a client has to be held to each: a stand-in
does what a test asks of it. What the client does with a real `serve run`
is in the tests that start one (`jobs.py`).

A stand-in answers a route asked as a WebSocket too, where a test told it
what the stream sends (`StandIn.streams`): frames as WebSocket writes them,
or whatever bytes a test would have a server send in their place.
"""

import base64
import hashlib
import http.server
import json
import struct
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


# The kinds of frame WebSocket has, and what a server's answer to an upgrade
# is taken over.
TEXT, BINARY, CLOSE, PING, PONG, CONTINUED = 1, 2, 8, 9, 10, 0
ACCEPT_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

# What a stream sends in the place of bytes to let go of its connection at
# once, as a server that is gone does, without hearing its client out.
DROP = object()


def frame(payload=b"", kind=TEXT, final=True, mask=None, reserved=0, length_in=None) -> bytes:
    """One frame as WebSocket writes it: a server's, which is not masked,
    unless a test gives it a mask of four bytes. A `dict` is the JSON of
    it, as every frame of the protocol is, and a `str` its UTF-8. Its
    length is written in the fewest bytes that hold it, as WebSocket has
    it, unless a test says in how many more: `length_in` 2 or 8."""
    if isinstance(payload, dict):
        payload = json.dumps(payload)
    if isinstance(payload, str):
        payload = payload.encode()
    head = bytes([(0x80 if final else 0) | reserved | kind])
    masked = 0x80 if mask else 0
    if length_in is None:
        length_in = 0 if len(payload) < 126 else 2 if len(payload) < 65536 else 8
    if length_in == 0:
        head += bytes([masked | len(payload)])
    elif length_in == 2:
        head += bytes([masked | 126]) + struct.pack(">H", len(payload))
    else:
        head += bytes([masked | 127]) + struct.pack(">Q", len(payload))
    if mask:
        payload = bytes(byte ^ mask[at % 4] for at, byte in enumerate(payload))
        head += mask
    return head + payload


def closing(code=1000, reason="") -> bytes:
    """The frame a server closes a stream with."""
    return frame(struct.pack(">H", code) + reason.encode(), CLOSE)


def accepting(key: str) -> str:
    """What a server answers a client's key with."""
    return base64.b64encode(hashlib.sha1((key + ACCEPT_GUID).encode()).digest()).decode()


def upgraded(key: str, **headers) -> bytes:
    """A server's answer to an upgrade it takes, with `headers` in the
    place of its own or beside them; one given as `None` is left out."""
    said = {
        "Upgrade": "websocket",
        "Connection": "Upgrade",
        "Sec-WebSocket-Accept": accepting(key),
    }
    said.update({name.replace("_", "-"): value for name, value in headers.items()})
    lines = [f"{name}: {value}" for name, value in said.items() if value is not None]
    return "\r\n".join(["HTTP/1.1 101 Switching Protocols", *lines, "", ""]).encode()


def heard(read) -> list[tuple[int, bytes]]:
    """What a client sent on a stream, to the end of its connection: each
    frame as its kind and its payload, which a client masks."""
    frames = []
    while True:
        head = read(2)
        if len(head) < 2:
            return frames
        length = head[1] & 0x7F
        assert head[0] & 0x80 and head[1] & 0x80 and length < 126, "a control frame, masked"
        mask = read(4)
        payload = bytes(byte ^ mask[at % 4] for at, byte in enumerate(read(length)))
        frames.append((head[0] & 0x0F, payload))


class StandIn:
    """A server of the test's own, on this machine: it answers each route
    with what the test told it to, one answer after another and the last
    one from then on, `GET /version` with `SPEAKS` unless told otherwise,
    and any other route as `answering` makes of it: as a QuantForge server
    answers one it does not have, unless a stand-in derived from this one
    says otherwise. An answer that is a function is called as it is asked
    for, which is how a test holds one back, and one that is bytes is sent
    as it is, whatever it is, on a connection that is then let go of.
    `asked` is what it was asked, in order."""

    def __init__(self, speaks=SPEAKS):
        self.asked = []
        self.heard = []
        self._answers = {("GET", "/version"): [(200, speaks, {})]}
        self._streams = {}
        # The targets whose last stream has been asked for.
        self._sent = set()
        stand_in = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def stream(self, sent):
                """Answers an upgrade, sends what the test said the stream
                sends, and keeps what the client sent until it let go."""
                self.close_connection = True
                key = self.headers.get("Sec-WebSocket-Key", "")
                self.connection.settimeout(30)
                try:
                    for each in sent:
                        if callable(each):
                            each = each(key)
                        if each is DROP:
                            stand_in.heard.append(DROP)
                            return
                        if each is not None:
                            self.wfile.write(each)
                            self.wfile.flush()
                    stand_in.heard.append(heard(self.rfile.read))
                except OSError:
                    stand_in.heard.append(None)

            def answer(self):
                length = int(self.headers.get("Content-Length") or 0)
                sent = self.rfile.read(length) if length else b""
                stand_in.asked.append((self.command, self.path, dict(self.headers), sent))
                streams = stand_in._streams.get(self.path)
                if streams and "Upgrade" in self.headers:
                    # Each in turn, and the last from then on, until the
                    # test says what the next one sends.
                    if len(streams) > 1:
                        return self.stream(streams.pop(0))
                    stand_in._sent.add(self.path)
                    return self.stream(streams[0])
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
                if isinstance(answer, bytes):
                    # Sent as it is, in the place of an answer written here,
                    # and the connection let go of after it.
                    self.close_connection = True
                    self.wfile.write(answer)
                    return
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
            answer
            if callable(answer) or isinstance(answer, bytes) or len(answer) == 3
            else (*answer, {})
            for answer in answers
        ]
        return self

    def streams(self, target: str, *sent, answer=upgraded) -> "StandIn":
        """From now on `GET target` asked as a WebSocket is answered as an
        upgrade it takes, and then sends `sent`, in order: each the bytes
        to send, or a function of the client's key that gives them, or
        gives nothing and does something else in their place, or `DROP`,
        at which it lets go of the connection. Once it has sent them it
        keeps what the client sends, to the end of the connection, as
        `heard`: the client's frames, `DROP` for a connection it let go of
        itself, and `None` for one that was cut off under it. `answer` is
        what it says to the upgrade, in the place of taking it. Called
        again for a target, it says what the next stream of that target
        sends, and the last from then on."""
        streams = self._streams.setdefault(target, [])
        if target in self._sent:
            # The one before it was the last until now, and has been sent.
            streams.clear()
            self._sent.discard(target)
        streams.append([answer, *sent])
        return self

    @property
    def routes(self) -> list[str]:
        """What it was asked, each as its method and its target, with
        `(stream)` after one that was asked as a WebSocket."""
        return [
            f"{method} {target}{' (stream)' if 'Upgrade' in headers else ''}"
            for method, target, headers, _ in self.asked
        ]

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
