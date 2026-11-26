"""The client of a QuantForge server, against servers of the test's own
(docs/python-client.md): what a client refuses before it asks anything,
which token it sends and where it never sends one, what it agrees on with a
server and when, what it raises for each way a server refuses a request,
what it asks again, and what it reads of an answer and refuses to.

A stand-in here answers what a test tells it to and records what it was
asked, so each test says exactly what a server said. No QuantForge server
checks a token yet: a stand-in that does is how a refused token is shown
to raise `AuthError`. What the client does with a real `serve run` and its
jobs is in `test_client_jobs.py`.
"""

import collections
import concurrent.futures
import copy
import http.client
import http.server
import io
import json
import pickle
import random
import re
import socket
import ssl
import subprocess
import sys
import textwrap
import threading
import types

import pytest

import quantforge_py as qf
from conftest import REMOTE_TOKEN
from quantforge_py import _answer, _errors, _jobs, _wire

# What a server of this release says of itself.
SPEAKS = {"version": "0.2.0", "proto_version": 1, "min_supported": 1}

# A token no line could hold by chance. It is words, and holds no digit, so
# that no scanner of the repository takes it for a secret someone left.
TOKEN = "a-sentinel-token-no-line-holds"

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
    and any other route as a QuantForge server answers one it does not
    have. An answer that is a function is called as it is asked for, which
    is how a test holds one back. `asked` is what it was asked, in order."""

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


class Raw:
    """A listener on this machine that does to each connection what the
    test gives it to do, and closes it. `heard` is what each connection
    sent before that."""

    def __init__(self, serve):
        self.heard = []
        self._serve = serve
        self._listener = socket.create_server(("127.0.0.1", 0))
        self._listener.settimeout(0.05)
        self.port = self._listener.getsockname()[1]
        self._stopped = threading.Event()
        self._thread = threading.Thread(target=self._accept, daemon=True)
        self._thread.start()

    def _accept(self):
        while not self._stopped.is_set():
            try:
                connection, _ = self._listener.accept()
            except TimeoutError:
                continue
            with connection:
                connection.settimeout(5)
                try:
                    self.heard.append(connection.recv(65536))
                    self._serve(connection)
                except OSError:
                    pass

    def close(self):
        self._stopped.set()
        self._thread.join()
        self._listener.close()

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.close()


def kept(sent: bytes):
    """What a `Raw` does to a connection: it sends this, and keeps the
    connection until the client closes it, as a server that serves more
    than one request on a connection does."""

    def serve(connection):
        connection.sendall(sent)
        try:
            connection.recv(1)
        except OSError:
            pass

    return serve


def in_chunks(*chunks: bytes) -> bytes:
    """A body as chunks write it: each with its size before it, and the
    last chunk, of no bytes, after them."""
    return b"".join(b"%x\r\n%b\r\n" % (len(chunk), chunk) for chunk in chunks) + b"0\r\n\r\n"


@pytest.fixture
def server():
    with StandIn() as stand_in:
        yield stand_in


@pytest.fixture
def no_waiting(monkeypatch):
    """Records how long the client would have waited between two tries, in
    the place of waiting."""
    waited = []
    # The client's own name for the clock, and no other module's.
    monkeypatch.setattr(_wire, "time", types.SimpleNamespace(sleep=waited.append))
    return waited


def raised(kind, call):
    """What a call raised: this exception itself, not one derived from
    it."""
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


# Before a client asks anything.


def test_a_client_asks_its_server_nothing_until_it_is_first_used(server):
    client = qf.Client(server.url)
    assert (client.url, repr(client)) == (f"{server.url}/", f"<Client {server.url}/>")
    assert (client.server_version, client.proto_version) == (None, None)
    assert repr(client.backtest) == f"<backtests of {server.url}/>"
    assert client.backtest is client.backtest
    assert server.asked == []
    # Its first use asks which versions the server speaks, and then what it
    # was used for, under the newest both speak.
    server.says("GET", "/v1/backtests", (200, {"jobs": []}))
    assert client.backtest.list() == []
    assert server.routes == ["GET /version", "GET /v1/backtests"]
    assert (client.server_version, client.proto_version) == ("0.2.0", 1)
    # A server that was asked is not asked again.
    assert client.backtest.list() == []
    assert client.handshake() is None
    assert server.routes == ["GET /version", "GET /v1/backtests", "GET /v1/backtests"]


def test_a_handshake_asks_the_server_now_and_once(server):
    client = qf.Client(server.url)
    assert client.handshake() is None
    assert (client.server_version, client.proto_version) == ("0.2.0", 1)
    client.handshake()
    assert server.routes == ["GET /version"]


def test_every_request_says_who_asks_and_goes_on_a_connection_of_its_own(server):
    server.says("GET", "/v1/backtests", (200, {"jobs": []}))
    qf.Client(server.url).backtest.list()
    for _, _, headers, sent in server.asked:
        assert headers["User-Agent"] == f"quantforge-py/{qf.__version__}"
        assert headers["Accept"] == "application/json"
        assert headers["Connection"] == "close"
        assert "Authorization" not in headers
        assert sent == b""


def test_a_server_under_a_path_is_asked_under_it(server):
    server.says("GET", "/lab/version", (200, SPEAKS)).says(
        "GET", "/lab/v1/backtests?limit=7", (200, {"jobs": []})
    )
    for written_as in (f"{server.url}/lab", f"{server.url}/lab/"):
        client = qf.Client(written_as)
        assert client.url == f"{server.url}/lab/"
        assert client.backtest.list(limit=7) == []
    assert server.routes == ["GET /lab/version", "GET /lab/v1/backtests?limit=7"] * 2


def test_a_client_is_used_by_many_threads_at_once(server):
    server.says("GET", "/v1/backtests", (200, {"jobs": [written()]}))
    server.says("GET", f"/v1/backtests/{JOB}", (200, written()))
    client = qf.Client(server.url)
    together = threading.Barrier(8)

    def use():
        together.wait(timeout=30)
        (job,) = client.backtest.list()
        return job.refresh().job_id

    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        uses = [pool.submit(use) for _ in range(8)]
        assert [each.result(timeout=30) for each in uses] == [JOB] * 8
    # Each request went on a connection of its own. The threads that used
    # the client first each asked the server for its versions, and none did
    # once one of them had its answer.
    assert server.routes.count("GET /v1/backtests") == 8
    assert server.routes.count(f"GET /v1/backtests/{JOB}") == 8
    asked = server.routes.count("GET /version")
    assert 1 <= asked <= 8
    client.backtest.list()
    assert server.routes.count("GET /version") == asked


@pytest.mark.parametrize(
    "written_as, url",
    [
        ("http://127.0.0.1:9600", "http://127.0.0.1:9600/"),
        ("  https://qf.example/lab  ", "https://qf.example/lab/"),
        ("HTTP://QF.Example:80/Lab/", "http://qf.example/Lab/"),
        ("https://qf.example:443", "https://qf.example/"),
        ("https://qf.example:8443", "https://qf.example:8443/"),
        ("http://[::1]:9600", "http://[::1]:9600/"),
        ("http://localhost:9600/a b".replace(" ", "%20"), "http://localhost:9600/a%20b/"),
        ("http://localhost/bücher", "http://localhost/b%C3%BCcher/"),
    ],
)
def test_a_servers_url_is_http_or_https_with_a_host_and_its_path_ends_with_a_slash(written_as, url):
    assert qf.Client(written_as).url == url


@pytest.mark.parametrize(
    "written_as, said",
    [
        ("", "base_url is empty; name the server, such as http://127.0.0.1:9600"),
        ("   ", "base_url is empty; name the server, such as http://127.0.0.1:9600"),
        (
            "127.0.0.1:9600",
            "base_url must start with http:// or https://, such as http://127.0.0.1:9600",
        ),
        (
            "localhost:9600",
            "base_url must start with http:// or https://, such as http://127.0.0.1:9600",
        ),
        (
            "ws://127.0.0.1:9600",
            "base_url must start with http:// or https://, such as http://127.0.0.1:9600",
        ),
        (
            "http:/127.0.0.1:9600",
            "base_url must start with http:// or https://, such as http://127.0.0.1:9600",
        ),
        ("http://", "base_url names no host; name the server, such as http://127.0.0.1:9600"),
        ("http:///lab", "base_url names no host; name the server, such as http://127.0.0.1:9600"),
        ("http://127.0.0.1:0", "base_url names port 0, which no server listens on"),
        (
            "http://127.0.0.1:port",
            "base_url is not a URL the client reads; name the server, such as "
            "http://127.0.0.1:9600",
        ),
        (
            "http://127.0.0.1:99999",
            "base_url is not a URL the client reads; name the server, such as "
            "http://127.0.0.1:9600",
        ),
        (
            "http://[::1",
            "base_url is not a URL the client reads; name the server, such as "
            "http://127.0.0.1:9600",
        ),
        (
            "http://127.0.0.1:9600/?x=1",
            "base_url holds a query or a fragment; name the server and the path it is served "
            "under, and nothing more",
        ),
        (
            "http://127.0.0.1:9600/#top",
            "base_url holds a query or a fragment; name the server and the path it is served "
            "under, and nothing more",
        ),
        (
            "http://127.0.0.1:9600/a b",
            "base_url holds a space or a character that is not printed; name the server, such as "
            "http://127.0.0.1:9600",
        ),
        (
            "http://127.0.0.1:9600/a\x1b[2Jb",
            "base_url holds a space or a character that is not printed; name the server, such as "
            "http://127.0.0.1:9600",
        ),
    ],
)
def test_what_is_no_servers_url_is_refused_before_anything_is_asked(written_as, said, refused):
    refused(ValueError, said, qf.Client, written_as)


def test_a_url_that_carries_a_password_is_refused_and_never_shown(refused):
    said = (
        "base_url holds a user name or a password, which the client does not send; give the "
        "server's token as token, or in QF_REMOTE_TOKEN"
    )
    for written_as in (
        f"https://ann:{TOKEN}@qf.example",
        f"https://{TOKEN}@qf.example/lab",
        f"http://:{TOKEN}@127.0.0.1:9600",
    ):
        refused(ValueError, said, qf.Client, written_as)
    # Nor is one the standard library cannot read.
    for written_as in (f"https://ann:{TOKEN}@[::1", f"https://ann:{TOKEN}@qf.example:port"):
        error = raised(ValueError, lambda: qf.Client(written_as))
        assert TOKEN not in str(error) and error.__cause__ is None and error.__context__ is None


@pytest.mark.parametrize("given", [b"http://127.0.0.1:9600", None, 9600, ["http://127.0.0.1"]])
def test_a_url_is_text(given, refused):
    refused(TypeError, f"base_url must be a str, not {type(given).__name__}", qf.Client, given)


# The token.


def authorization(server) -> list[str | None]:
    """The `Authorization` of each request the stand-in was asked."""
    return [headers.get("Authorization") for _, _, headers, _ in server.asked]


def test_the_token_is_the_argument_then_the_variable_then_none(server, monkeypatch):
    def sent(*arguments, **named):
        """What a client built so sends with a request."""
        del server.asked[:]
        qf.Client(server.url, *arguments, **named).handshake()
        (header,) = authorization(server)
        return header

    # Neither: no token.
    assert sent() is None
    # The variable, where the argument is left out or `None`.
    monkeypatch.setenv(REMOTE_TOKEN, "from-the-variable")
    assert sent() == "Bearer from-the-variable"
    assert sent(None) == "Bearer from-the-variable"
    assert sent(token=None) == "Bearer from-the-variable"
    # The argument over the variable, by name or by place.
    assert sent(token="from-the-argument") == "Bearer from-the-argument"
    assert sent("from-the-argument") == "Bearer from-the-argument"
    # And the argument alone.
    monkeypatch.delenv(REMOTE_TOKEN)
    assert sent(token="from-the-argument") == "Bearer from-the-argument"


def test_an_empty_token_is_none_and_an_empty_argument_never_falls_back_to_the_variable(
    server, monkeypatch
):
    def sent(**named):
        del server.asked[:]
        client = qf.Client(server.url, **named)
        client.handshake()
        (header,) = authorization(server)
        return header, repr(client)

    without = (None, f"<Client {server.url}/>")
    for empty in ("", " ", "\t\n"):
        assert sent(token=empty) == without
    monkeypatch.setenv(REMOTE_TOKEN, "from-the-variable")
    # An argument that is given is the token, though it is none.
    for empty in ("", "   "):
        assert sent(token=empty) == without
    # An empty variable is no token either.
    for empty in ("", "   "):
        monkeypatch.setenv(REMOTE_TOKEN, empty)
        assert sent() == without
    # The space around a token is not part of it.
    monkeypatch.setenv(REMOTE_TOKEN, "  from-the-variable \n")
    assert sent() == ("Bearer from-the-variable", f"<Client {server.url}/, with a token>")
    assert sent(token=" from-the-argument ")[0] == "Bearer from-the-argument"


def test_the_variable_is_read_as_a_client_is_made_and_not_before_or_after(server, monkeypatch):
    client = qf.Client(server.url)
    monkeypatch.setenv(REMOTE_TOKEN, "set-afterwards")
    client.handshake()
    assert authorization(server) == [None]
    later = qf.Client(server.url)
    monkeypatch.delenv(REMOTE_TOKEN)
    later.handshake()
    assert authorization(server) == [None, "Bearer set-afterwards"]


def test_the_token_goes_with_every_request_and_is_never_shown(server):
    listed = {"jobs": [written(state={"kind": "running", "started_at_ms": 1})]}
    server.says("GET", "/v1/backtests", (200, listed))
    server.says("GET", f"/v1/backtests/{JOB}", (200, listed["jobs"][0]))
    server.says("POST", f"/v1/backtests/{JOB}/cancel", refusal(409, "conflict", "it has ended"))
    client = qf.Client(server.url, token=TOKEN)
    (job,) = client.backtest.list()
    job.refresh()
    error = raised(qf.Conflict, job.cancel)
    assert authorization(server) == [f"Bearer {TOKEN}"] * 4
    assert server.routes == [
        "GET /version",
        "GET /v1/backtests",
        f"GET /v1/backtests/{JOB}",
        f"POST /v1/backtests/{JOB}/cancel",
    ]
    shown = [client, client.backtest, job, job.status, error]
    for each in shown:
        assert TOKEN not in repr(each) and TOKEN not in str(each), each
    assert repr(client) == f"<Client {server.url}/, with a token>"
    # Nor where the server is not there, or is no QuantForge server.
    server.says("GET", "/v1/backtests", (200, "<html>hello</html>", {}))
    unreadable = raised(qf.RemoteError, client.backtest.list)
    server.close()
    unreachable = raised(qf.ServerUnavailable, client.backtest.list)
    for error in (unreadable, unreachable):
        assert TOKEN not in str(error) and TOKEN not in repr(error.__cause__), error


@pytest.mark.parametrize(
    "token", ["with space", "naïve", "tab\there", "line\nbreak", "\x7f", "né"]
)
def test_a_token_that_cannot_be_a_headers_value_is_refused_and_never_shown(
    token, monkeypatch, refused
):
    said = (
        "{} holds a character other than printable ASCII, or a space inside it: a token is sent "
        "in a header"
    )
    refused(ValueError, said.format("token"), qf.Client, "http://127.0.0.1:9600", token=token)
    monkeypatch.setenv(REMOTE_TOKEN, token)
    refused(ValueError, said.format(REMOTE_TOKEN), qf.Client, "http://127.0.0.1:9600")
    # An argument is the token, and the variable is not so much as read.
    assert repr(qf.Client("http://127.0.0.1:9600", token="t")).endswith(", with a token>")


@pytest.mark.parametrize("token", [b"secret", 1, True, ["secret"]])
def test_a_token_is_text(token, refused):
    said = f"token must be a str or None, not {type(token).__name__}"
    refused(TypeError, said, qf.Client, "http://127.0.0.1:9600", token=token)


NOT_THIS_MACHINE = [
    "http://qf.example:9600",
    "http://10.0.0.5:9600",
    "http://[2001:db8::1]",
    "http://localhost.example",
    "http://127.0.0.1.example",
]


@pytest.mark.parametrize("url", NOT_THIS_MACHINE)
def test_a_token_never_crosses_the_network_in_clear_text(url, monkeypatch, refused):
    shown = qf.Client(url).url
    said = (
        "a token goes only over https, or over http to this machine, and the server at "
        f"{shown} is neither; nothing was sent"
    )
    refused(ValueError, said, qf.Client, url, token=TOKEN)
    # One the variable holds is said to be the variable's, with how a client
    # is made without it.
    monkeypatch.setenv(REMOTE_TOKEN, TOKEN)
    exported = f'{said}. The token is QF_REMOTE_TOKEN\'s: pass token="" to send none'
    refused(ValueError, exported, qf.Client, url)
    # The way out the refusal names: no token, though the variable holds one.
    assert repr(qf.Client(url, token="")) == f"<Client {shown}>"
    # Over https it goes anywhere.
    secure = url.replace("http://", "https://")
    assert repr(qf.Client(secure)).endswith(", with a token>")


@pytest.mark.parametrize(
    "url",
    [
        "http://127.0.0.1:9600",
        "http://127.8.9.10",
        "http://[::1]:9600",
        "http://localhost:9600",
        "http://LOCALHOST",
    ],
)
def test_a_token_goes_over_http_to_this_machine(url):
    assert repr(qf.Client(url, token=TOKEN)).endswith(", with a token>")


def test_a_token_goes_over_http_only_on_a_connection_that_ends_on_this_machine(
    server, monkeypatch
):
    # `localhost` is this machine by its name, and a name leads where the
    # system's resolver says. Here it is made to lead to the address the
    # stand-in listens on, whatever the machine's own resolver makes of it,
    # and the token goes.
    resolve = socket.getaddrinfo

    def to_the_stand_in(host, *rest, **named):
        return resolve("127.0.0.1" if host == "localhost" else host, *rest, **named)

    monkeypatch.setattr(socket, "getaddrinfo", to_the_stand_in)
    url = f"http://localhost:{server.port}"
    qf.Client(url, token=TOKEN).handshake()
    assert authorization(server) == [f"Bearer {TOKEN}"]
    # Where it leads elsewhere, the connection is given up before anything
    # is sent on it: the stand-in is reached, and told nothing.
    with_a_token, without_one = qf.Client(url, token=TOKEN), qf.Client(url)
    monkeypatch.setattr(_wire, "_loopback", lambda address: False)
    for use in (with_a_token.handshake, with_a_token.backtest.list):
        error = raised(qf.RemoteError, use)
        assert str(error) == (
            f"the server at http://localhost:{server.port}/ is not on this machine: its name "
            "leads to 127.0.0.1 here, and a token goes over http to this machine alone; nothing "
            "was sent"
        )
        assert (error.code, error.status) == (None, None) and TOKEN not in str(error)
    assert len(server.asked) == 1
    # The rule is the token's: a client without one asks wherever the name
    # leads.
    without_one.handshake()
    assert authorization(server) == [f"Bearer {TOKEN}", None]


@pytest.mark.parametrize(
    "address, this_machine",
    [
        ("127.0.0.1", True),
        ("127.8.9.10", True),
        ("::1", True),
        ("::ffff:127.0.0.1", True),
        ("10.0.0.5", False),
        ("203.0.113.7", False),
        ("::ffff:10.0.0.5", False),
        ("2001:db8::1", False),
        ("fe80::1%en0", False),
        ("0.0.0.0", False),
        ("localhost", False),
        ("", False),
    ],
)
def test_this_machine_is_a_loopback_address_and_no_other(address, this_machine):
    assert _wire._loopback(address) is this_machine


def test_what_tls_cannot_be_made_ready_with_is_a_server_that_was_not_reached(
    monkeypatch, tmp_path
):
    # Making ready for TLS reads files, and fails as opening a connection
    # does: before one is opened, so no server is asked for here.
    def unreadable():
        raise FileNotFoundError(2, "No such file or directory", "/no/such/keys.log")

    with pytest.MonkeyPatch.context() as patched:
        patched.setattr(_wire, "_tls_context", unreadable)
        client = qf.Client("https://qf.example", token=TOKEN)
        for use in (client.handshake, client.backtest.list):
            error = raised(qf.ServerUnavailable, use)
            assert str(error) == (
                "could not reach the server at https://qf.example/: FileNotFoundError: [Errno 2] "
                "No such file or directory: '/no/such/keys.log'"
            )
            assert isinstance(error.__cause__, FileNotFoundError)
            assert (error.code, error.status) == (None, None)
    # And as Python itself fails at it, where its TLS writes a key log: one
    # that cannot be opened. The server named is a port of this machine that
    # nothing listens on, so that nothing leaves it either way.
    if hasattr(ssl.SSLContext, "keylog_filename") and not sys.flags.ignore_environment:
        monkeypatch.setenv("SSLKEYLOGFILE", str(tmp_path / "no" / "such" / "keys.log"))
        url = f"https://127.0.0.1:{nobody_listens()}"
        error = raised(qf.ServerUnavailable, qf.Client(url).handshake)
        assert str(error).startswith(f"could not reach the server at {url}/: FileNotFoundError")
        assert isinstance(error.__cause__, FileNotFoundError)


def test_over_https_a_certificate_is_checked_and_nothing_is_said_in_clear_text():
    # How a certificate is checked: against what the system trusts, and the
    # name the URL gives.
    context = _wire._tls_context()
    assert context.verify_mode == ssl.CERT_REQUIRED and context.check_hostname
    # A listener that speaks no TLS is told nothing: not a request, not the
    # token. What it hears is the start of a TLS handshake, and what it says
    # back in clear text ends the connection there.
    with Raw(lambda connection: connection.sendall(b"HTTP/1.0 200 OK\r\n\r\n{}")) as plain:
        client = qf.Client(f"https://127.0.0.1:{plain.port}", token=TOKEN)
        error = raised(qf.ServerUnavailable, client.handshake)
        heard = b"".join(plain.heard)
    assert str(error).startswith(f"could not reach the server at https://127.0.0.1:{plain.port}/: ")
    # The handshake's own failure, or the reset of a listener that hung up
    # in the middle of one.
    assert isinstance(error.__cause__, (ssl.SSLError, ConnectionError)), error.__cause__
    assert (error.code, error.status) == (None, None)
    assert (client.server_version, client.proto_version) == (None, None)
    handshake_record = b"\x16"
    assert heard[:1] == handshake_record
    assert TOKEN.encode() not in heard and b"GET" not in heard and b"Bearer" not in heard


# The versions.


@pytest.mark.parametrize(
    "speaks, agreed",
    [
        ({"version": "0.2.0", "proto_version": 1, "min_supported": 1}, 1),
        ({"version": "0.9.0", "proto_version": 4, "min_supported": 1}, 1),
        # What a later release says beside them is left unread.
        ({"version": "1.0.0", "proto_version": 2, "min_supported": 1, "runtime": {"x": []}}, 1),
    ],
)
def test_a_client_agrees_on_the_newest_version_both_sides_speak(speaks, agreed):
    with StandIn(speaks) as server:
        server.says("GET", f"/v{agreed}/backtests", (200, {"jobs": []}))
        client = qf.Client(server.url)
        assert client.backtest.list() == []
        assert (client.server_version, client.proto_version) == (speaks["version"], agreed)


def test_a_server_that_speaks_no_version_of_the_package_is_a_version_mismatch():
    with StandIn({"version": "0.9.0", "proto_version": 4, "min_supported": 3}) as server:
        client = qf.Client(server.url)
        for use in (client.handshake, client.backtest.list, lambda: client.backtest.get(JOB)):
            error = raised(qf.VersionMismatch, use)
            assert str(error) == (
                f"the server at {server.url}/ runs QuantForge 0.9.0, which speaks protocol "
                "versions 3 to 4, and this package speaks 1 to 1; upgrade the older of the two"
            )
            assert (error.supported, error.code, error.status) == ((3, 4), None, None)
        # Nothing was asked under a version, and nothing was agreed.
        assert server.routes == ["GET /version"] * 3
        assert (client.server_version, client.proto_version) == (None, None)


def test_the_client_speaks_the_protocol_versions_the_engine_does(repository):
    source = (repository / "src" / "protocol" / "version.rs").read_text(encoding="utf-8")
    spoken = {
        name: int(version)
        for name, version in re.findall(
            r"^pub const (\w+): ProtoVersion = ProtoVersion\((\d+)\);$", source, re.M
        )
    }
    assert spoken == {
        "PROTO_VERSION": _wire._PROTO_VERSION,
        "MIN_SUPPORTED": _wire._MIN_SUPPORTED,
    }


@pytest.mark.parametrize(
    "answer, why",
    [
        ("<html>hello</html>", "it is not JSON"),
        ("", "it is not JSON"),
        (b"\xff\xfe", "it is not JSON"),
        ('{"version":"0.2.0","proto_version":NaN,"min_supported":1}', "it is not JSON"),
        ("[" * 100_000, "it is not JSON"),
        ([SPEAKS], "the server's version is not an object"),
        ({"proto_version": 1, "min_supported": 1}, "the server's version has no `version`"),
        ({"version": "0.2.0", "min_supported": 1}, "the server's version has no `proto_version`"),
        ({"version": "0.2.0", "proto_version": 1}, "the server's version has no `min_supported`"),
        (
            {"version": 2, "proto_version": 1, "min_supported": 1},
            "the `version` of the server's version is not a string",
        ),
        (
            {"version": "0.2.0", "proto_version": "1", "min_supported": 1},
            "the `proto_version` of the server's version is not an integer",
        ),
        (
            {"version": "0.2.0", "proto_version": 1.0, "min_supported": 1},
            "the `proto_version` of the server's version is not an integer",
        ),
        (
            {"version": "0.2.0", "proto_version": True, "min_supported": 1},
            "the `proto_version` of the server's version is not an integer",
        ),
        (
            {"version": "0.2.0", "proto_version": 1, "min_supported": 0},
            "the server's version names the versions 0 to 1, which hold none",
        ),
        (
            {"version": "0.2.0", "proto_version": 2, "min_supported": 3},
            "the server's version names the versions 3 to 2, which hold none",
        ),
        (
            {"version": "0.2.0", "proto_version": 2**63, "min_supported": 1},
            "the `proto_version` of the server's version is beyond the integers the protocol holds",
        ),
    ],
)
def test_what_is_no_quantforge_servers_version_is_named_as_such(answer, why):
    with StandIn() as server:
        server.says("GET", "/version", (200, answer))
        error = raised(qf.RemoteError, qf.Client(server.url).handshake)
    assert str(error) == (
        f"the server at {server.url}/ answered GET /version with a body the protocol does not "
        f"read: {why}; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)


def test_a_version_the_server_no_longer_speaks_is_agreed_on_again(server):
    client = qf.Client(server.url)
    client.handshake()
    said = "this server speaks protocol versions 2 to 3, not 1"
    supported = {"min_supported": 2, "proto_version": 3}
    server.says(
        "GET", "/v1/backtests", refusal(400, "unsupported_version", said, supported=supported)
    )
    error = raised(qf.VersionMismatch, client.backtest.list)
    assert (str(error), error.code, error.status) == (said, "unsupported_version", 400)
    assert error.supported == (2, 3)
    # What was agreed is forgotten, and the next use asks the server again.
    assert (client.server_version, client.proto_version) == (None, None)
    server.says("GET", "/v1/backtests", (200, {"jobs": []}))
    assert client.backtest.list() == []
    assert server.routes == ["GET /version", "GET /v1/backtests"] * 2
    # A refusal that lists no versions says so.
    server.says("GET", "/v1/backtests", refusal(400, "unsupported_version", said))
    assert raised(qf.VersionMismatch, client.backtest.list).supported is None


def test_an_answer_that_was_on_its_way_while_the_server_refused_a_version_is_not_kept(server):
    # Two threads use a client that has agreed on nothing yet. The first
    # asks the server which versions it speaks, and its answer is held back.
    asked, let_go = threading.Event(), threading.Event()

    def late():
        asked.set()
        assert let_go.wait(30)
        return 200, SPEAKS, {}

    said = "this server speaks protocol versions 2 to 3, not 1"
    supported = {"min_supported": 2, "proto_version": 3}
    server.says("GET", "/version", late, (200, SPEAKS))
    server.says(
        "GET",
        "/v1/backtests",
        refusal(400, "unsupported_version", said, supported=supported),
        (200, {"jobs": []}),
    )
    client = qf.Client(server.url)
    with concurrent.futures.ThreadPoolExecutor(1) as pool:
        first = pool.submit(client.backtest.list)
        assert asked.wait(30)
        # Meanwhile the second agrees on a version, and is refused it.
        assert str(raised(qf.VersionMismatch, client.backtest.list)) == said
        assert (client.server_version, client.proto_version) == (None, None)
        # Now the first is answered: with what the server said before it
        # refused. It serves the request it was asked for, and is not kept.
        let_go.set()
        assert first.result(timeout=30) == []
    assert (client.server_version, client.proto_version) == (None, None)
    assert server.routes == ["GET /version"] * 2 + ["GET /v1/backtests"] * 2
    # So the next use asks the server which versions it speaks now.
    assert client.backtest.list() == []
    assert server.routes[4:] == ["GET /version", "GET /v1/backtests"]
    assert (client.server_version, client.proto_version) == ("0.2.0", 1)


def test_an_answer_to_a_request_sent_after_the_refusal_is_the_servers_latest_and_is_kept(
    server, no_waiting
):
    # The first thread's question is answered late, and with a server that
    # is busy: it is asked again, and by then the server has refused a
    # version to the second thread.
    asked, let_go = threading.Event(), threading.Event()

    def busy_and_late():
        asked.set()
        assert let_go.wait(30)
        return busy("0")

    said = "this server speaks protocol versions 2 to 3, not 1"
    supported = {"min_supported": 2, "proto_version": 3}
    server.says("GET", "/version", busy_and_late, (200, SPEAKS))
    server.says(
        "GET",
        "/v1/backtests",
        refusal(400, "unsupported_version", said, supported=supported),
        (200, {"jobs": []}),
    )
    client = qf.Client(server.url)
    with concurrent.futures.ThreadPoolExecutor(1) as pool:
        first = pool.submit(client.handshake)
        assert asked.wait(30)
        raised(qf.VersionMismatch, client.backtest.list)
        assert (client.server_version, client.proto_version) == (None, None)
        let_go.set()
        assert first.result(timeout=30) is None
    # What the first was answered then, it had asked after the refusal: a
    # handshake that returns has agreed, and the client says on what.
    assert server.routes == ["GET /version"] * 2 + ["GET /v1/backtests", "GET /version"]
    assert (client.server_version, client.proto_version) == ("0.2.0", 1)
    assert client.backtest.list() == []
    assert server.routes[4:] == ["GET /v1/backtests"]


# What a server refuses a request with.


# The exception each code is raised as. A code with none of its own is a
# `RemoteError`, and so is one this version of the package does not know.
RAISED = {
    "invalid_request": qf.ValidationError,
    "unauthorized": qf.AuthError,
    "not_found": qf.NotFound,
    "method_not_allowed": qf.RemoteError,
    "conflict": qf.Conflict,
    "unsupported_version": qf.VersionMismatch,
    "limit_exceeded": qf.RemoteError,
    "rate_limited": qf.RemoteError,
    "unavailable": qf.ServerUnavailable,
    "internal": qf.RemoteError,
}


def test_every_code_of_the_protocol_is_raised_as_its_exception_with_the_servers_message(
    server, protocol_codes
):
    codes = protocol_codes
    assert list(codes) == list(RAISED)
    assert _errors._RAISED == {
        code: kind for code, kind in RAISED.items() if kind is not qf.RemoteError
    }
    client = qf.Client(server.url)
    # The server's words, whatever they hold: no word is added or changed.
    message = 'the range spans 9 bars of 1m, more than the 5 this server runs one job on; "ask" ⏱'
    later = {"quota_exhausted": 402, "teapot": 418}
    for code, status in {**codes, **later}.items():
        server.says("GET", "/v1/backtests", refusal(status, code, message))
        error = raised(RAISED.get(code, qf.RemoteError), client.backtest.list)
        assert (str(error), error.args) == (message, (message,)), code
        assert (error.code, error.status) == (code, status), code
        assert isinstance(error, qf.RemoteError) and isinstance(error, Exception)
    # Each was asked once, as none of them asks to be asked again; and the
    # server was asked for its versions again once it had refused the one
    # agreed on.
    asked = ["GET /v1/backtests"] * 6
    assert list(codes)[5] == "unsupported_version"
    assert server.routes == ["GET /version", *asked, "GET /version", *asked]
    # A code is what a refusal is raised by, and never its status.
    server.says("GET", "/v1/backtests", refusal(500, "not_found", message))
    assert raised(qf.NotFound, client.backtest.list).status == 500


class Checking(StandIn):
    """A stand-in that checks the token of every request, as a server will:
    one that is missing or is not `expects` is refused, `unauthorized`."""

    def __init__(self, expects: str):
        super().__init__()
        self.expects = expects
        handler = self.server.RequestHandlerClass
        answer = handler.answer
        checking = self

        def checked(self):
            given = self.headers.get("Authorization")
            if given == f"Bearer {checking.expects}":
                return answer(self)
            checking.asked.append((self.command, self.path, dict(self.headers), b""))
            said = (
                "this server takes a token, and the request carries none"
                if given is None
                else "the token is unknown or has expired"
            )
            body = json.dumps({"error": {"code": "unauthorized", "message": said}}).encode()
            self.send_response(401)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        handler.do_GET = handler.do_POST = checked


def test_a_refused_token_is_an_auth_error_that_says_what_the_server_said(monkeypatch):
    with Checking(TOKEN) as server:
        cancelled = written(state={"kind": "cancelled", "finished_at_ms": 9})
        server.says("GET", "/v1/backtests", (200, {"jobs": [written()]}))
        server.says("POST", f"/v1/backtests/{JOB}/cancel", (200, cancelled))
        # No token, a wrong one, and a wrong one from the variable.
        without = raised(qf.AuthError, qf.Client(server.url).backtest.list)
        assert str(without) == "this server takes a token, and the request carries none"
        wrong = raised(qf.AuthError, qf.Client(server.url, token="not-the-token").backtest.list)
        assert str(wrong) == "the token is unknown or has expired"
        monkeypatch.setenv(REMOTE_TOKEN, "not-the-token-either")
        exported = raised(qf.AuthError, qf.Client(server.url).handshake)
        assert str(exported) == "the token is unknown or has expired"
        for error in (without, wrong, exported):
            assert (error.code, error.status) == ("unauthorized", 401)
        # Each was refused at its first request, and nothing was agreed.
        assert server.routes == ["GET /version"] * 3
        # The right one is let through, to every route.
        client = qf.Client(server.url, token=TOKEN)
        (job,) = client.backtest.list()
        assert job.cancel().state == "cancelled"
        # And once it is no longer the server's, the client's next request
        # says so.
        server.expects = "another"
        assert str(raised(qf.AuthError, job.refresh)) == "the token is unknown or has expired"


@pytest.mark.parametrize(
    "status, phrase, kind",
    [
        (400, "Bad Request", qf.RemoteError),
        (401, "Unauthorized", qf.AuthError),
        (403, "Forbidden", qf.AuthError),
        (404, "Not Found", qf.RemoteError),
        (500, "Internal Server Error", qf.RemoteError),
        (502, "Bad Gateway", qf.ServerUnavailable),
        (503, "Service Unavailable", qf.ServerUnavailable),
        (504, "Gateway Timeout", qf.ServerUnavailable),
    ],
)
def test_a_refusal_without_the_protocols_envelope_is_told_by_its_status(status, phrase, kind):
    bodies = [
        "<html>nginx</html>",
        "",
        "{}",
        '{"error":"denied"}',
        '{"error":{"code":"not_found"}}',
        '{"error":{"code":7,"message":"m"}}',
        '{"error":{"code":"unsupported_version","message":"m","supported":{"min_supported":2}}}',
    ]
    with StandIn() as server:
        for body in bodies:
            server.says("GET", "/version", (status, body))
            error = raised(kind, qf.Client(server.url).handshake)
            assert str(error) == (
                f"the server at {server.url}/ answered {status} {phrase} without QuantForge's "
                "error envelope; is it a QuantForge server?"
            )
            assert (error.code, error.status) == (None, status)


@pytest.mark.parametrize(
    "status, phrase", [(301, "Moved Permanently"), (302, "Found"), (307, "Temporary Redirect")]
)
def test_a_redirect_is_never_followed(status, phrase):
    with StandIn() as server:
        server.says("GET", "/version", (status, "", {"Location": "/moved"}))
        server.says("GET", "/moved", (200, SPEAKS))
        error = raised(qf.RemoteError, qf.Client(server.url, token=TOKEN).handshake)
        assert str(error) == (
            f"the server at {server.url}/ answered {status} {phrase}, a redirect, which the "
            "client never follows; name the server by the URL it answers at"
        )
        assert (error.code, error.status) == (None, status)
        assert server.routes == ["GET /version"]


# What is asked again.


def busy(retry_after=None):
    headers = {} if retry_after is None else {"Retry-After": retry_after}
    said = "the server's store could not answer in time; retry later"
    return refusal(503, "unavailable", said, headers)


def test_a_request_is_asked_again_while_the_server_is_busy(server, no_waiting):
    client = qf.Client(server.url)
    # The handshake, a read and a cancel, each once the server stopped
    # saying that it was busy.
    server.says("GET", "/version", busy("0"), busy("1"), (200, SPEAKS))
    client.handshake()
    server.says("GET", "/v1/backtests", busy("0"), (200, {"jobs": [written()]}))
    (job,) = client.backtest.list()
    cancelled = written(state={"kind": "cancelled", "finished_at_ms": 9})
    server.says(
        "POST", f"/v1/backtests/{JOB}/cancel", busy("2"), busy("2"), busy("2"), (200, cancelled)
    )
    assert job.cancel().state == "cancelled"
    assert server.routes == (
        ["GET /version"] * 3 + ["GET /v1/backtests"] * 2 + [f"POST /v1/backtests/{JOB}/cancel"] * 4
    )
    # It waited as long as it was asked to, each time.
    assert no_waiting == [0, 1, 0, 2, 2, 2]


def test_a_server_that_stays_busy_is_unavailable_after_five_tries(server, no_waiting):
    client = qf.Client(server.url)
    client.handshake()
    # Busy for longer than the client asks, and then not: a client that went
    # on asking would be answered, and raise nothing.
    server.says("GET", "/v1/backtests", *[busy("1")] * 8, (200, {"jobs": []}))
    error = raised(qf.ServerUnavailable, client.backtest.list)
    assert str(error) == "the server's store could not answer in time; retry later"
    assert (error.code, error.status) == ("unavailable", 503)
    assert server.routes == ["GET /version"] + ["GET /v1/backtests"] * 5
    assert no_waiting == [1, 1, 1, 1]


@pytest.mark.parametrize(
    "retry_after",
    # The last but one is a digit to Python and no number: a superscript two.
    [None, "", "soon", "1.5", "-1", "+1", "1 s", "\xb2", "Wed, 21 Oct 2026 07:28:00 GMT"],
)
def test_a_busy_server_that_names_no_seconds_is_not_asked_again(server, no_waiting, retry_after):
    client = qf.Client(server.url)
    client.handshake()
    server.says("GET", "/v1/backtests", busy(retry_after))
    raised(qf.ServerUnavailable, client.backtest.list)
    assert server.routes == ["GET /version", "GET /v1/backtests"]
    assert no_waiting == []


def test_a_wait_is_five_seconds_at_most_and_a_proxys_503_is_waited_for_too(server, no_waiting):
    client = qf.Client(server.url)
    client.handshake()
    proxy = (503, "<html>busy</html>", {"Retry-After": "3600"})
    server.says("GET", "/v1/backtests", proxy, busy(" 7 "), (200, {"jobs": []}))
    assert client.backtest.list() == []
    assert no_waiting == [5, 5]
    # Any other refusal is raised at once, whatever it says of waiting.
    limited = refusal(429, "rate_limited", "too many requests", {"Retry-After": "1"})
    server.says("GET", "/v1/backtests", limited)
    raised(qf.RemoteError, client.backtest.list)
    assert no_waiting == [5, 5]


def test_a_wait_written_in_more_digits_than_python_reads_is_five_seconds_too(server, no_waiting):
    endless, padded, nothing = "9" * 5000, "0" * 5000 + "3", "0" * 5000
    if 0 < sys.get_int_max_str_digits() < len(endless):
        # Python refuses to read a number so long, unless it was told to
        # read longer ones or any, which is why none is converted.
        with pytest.raises(ValueError):
            int(endless)
    client = qf.Client(server.url)
    client.handshake()
    server.says(
        "GET", "/v1/backtests", busy(endless), busy(padded), busy(nothing), (200, {"jobs": []})
    )
    assert client.backtest.list() == []
    # The longest is the most that is waited, and zeros before a number are
    # not its digits.
    assert no_waiting == [5, 3, 0]
    assert server.routes == ["GET /version"] + ["GET /v1/backtests"] * 4


# A server that is not there, or is no server.


def nobody_listens() -> int:
    """A port of this machine nothing listens on."""
    with socket.create_server(("127.0.0.1", 0)) as listener:
        return listener.getsockname()[1]


def test_a_server_nobody_listens_for_is_unavailable():
    port = nobody_listens()
    client = qf.Client(f"http://127.0.0.1:{port}")
    for use in (client.handshake, client.backtest.list, lambda: client.backtest.get(JOB)):
        error = raised(qf.ServerUnavailable, use)
        assert str(error).startswith(
            f"could not reach the server at http://127.0.0.1:{port}/: ConnectionRefusedError"
        )
        assert isinstance(error.__cause__, ConnectionRefusedError)
        assert (error.code, error.status) == (None, None)


def test_a_connection_that_breaks_before_its_answer_is_unavailable():
    def hang_up(connection):
        pass

    def cut_short(connection):
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")

    whole = json.dumps(SPEAKS).encode()

    def cut_short_of_valid_json(connection):
        # What came is an answer of its own, and still not the one sent.
        head = f"HTTP/1.1 200 OK\r\nContent-Length: {len(whole) + 1}\r\n\r\n".encode()
        connection.sendall(head + whole)

    def cut_short_in_a_chunk(connection):
        connection.sendall(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n64\r\n{")

    def say_nothing(connection):
        connection.recv(1)

    for serve, cause in [
        (hang_up, "RemoteDisconnected: Remote end closed connection without response"),
        (cut_short, "IncompleteRead: IncompleteRead(1 bytes read, 99 more expected)"),
        (
            cut_short_of_valid_json,
            f"IncompleteRead: IncompleteRead({len(whole)} bytes read, 1 more expected)",
        ),
        (cut_short_in_a_chunk, "IncompleteRead: IncompleteRead("),
        (say_nothing, "TimeoutError: timed out"),
    ]:
        with Raw(serve) as broken, pytest.MonkeyPatch.context() as patched:
            if serve is say_nothing:
                # The one that is waited for: a fifth of a second, in the
                # place of a minute.
                patched.setattr(_wire, "_READ_SECONDS", 0.2)
            client = qf.Client(f"http://127.0.0.1:{broken.port}")
            error = raised(qf.ServerUnavailable, client.handshake)
        reached = f"could not reach the server at http://127.0.0.1:{broken.port}/: "
        assert str(error).startswith(reached + cause), serve.__name__
        assert cause.endswith("(") or str(error) == reached + cause, serve.__name__
        assert error.__cause__ is not None and (error.code, error.status) == (None, None)
        assert (client.server_version, client.proto_version) == (None, None)


def test_what_does_not_answer_in_http_is_named_as_no_quantforge_server():
    with Raw(lambda connection: connection.sendall(b"SSH-2.0-OpenSSH_9.9\r\n")) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
        "HTTP (BadStatusLine); is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)


def test_an_answer_longer_than_any_quantforge_server_sends_is_refused(server, monkeypatch):
    assert _answer._MOST_ANSWER_BYTES == 64 * 1024 * 1024
    monkeypatch.setattr(_answer, "_MOST_ANSWER_BYTES", 256)
    client = qf.Client(server.url)
    client.handshake()
    too_long = (
        f"the server at {server.url}/ answered GET /v1/backtests with more than 256 bytes, "
        "more than a QuantForge server sends; is it one?"
    )
    server.says("GET", "/v1/backtests", (200, "x" * 257))
    error = raised(qf.RemoteError, client.backtest.list)
    assert (str(error), error.code, error.status, error.__cause__) == (too_long, None, None, None)
    # A refusal is told by its status all the same: its body is read for the
    # protocol's error envelope alone, and one that is too long has none.
    server.says("GET", "/v1/backtests", (500, "x" * 257))
    error = raised(qf.RemoteError, client.backtest.list)
    assert str(error) == (
        f"the server at {server.url}/ answered 500 Internal Server Error without QuantForge's "
        "error envelope; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, 500)
    assert type(error.__cause__) is qf.RemoteError and str(error.__cause__) == too_long
    # As long as the longest is taken.
    server.says("GET", "/v1/backtests", (200, json.dumps({"jobs": []}).ljust(256)))
    assert client.backtest.list() == []


def test_an_answer_is_held_to_the_length_it_states_however_it_is_written(monkeypatch):
    whole = json.dumps(SPEAKS).encode()

    def endless(connection):
        head = b"HTTP/1.1 200 OK\r\nContent-Length: " + b"9" * 5000 + b"\r\n\r\n"
        connection.sendall(head + whole)

    def padded(connection):
        stated = b"0" * 5000 + str(len(whole)).encode()
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: " + stated + b"\r\n\r\n" + whole)

    def padded_and_cut_short(connection):
        stated = b"0" * 5000 + str(len(whole) + 1).encode()
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: " + stated + b"\r\n\r\n" + whole)

    def unstated(connection):
        connection.sendall(b"HTTP/1.0 200 OK\r\n\r\n" + whole.ljust(300))

    def cut_short_beside_an_encoding_that_is_none(connection):
        head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: identity\r\n"
        stated = b"Content-Length: " + str(len(whole) + 1).encode() + b"\r\n\r\n"
        connection.sendall(head + stated + whole)

    # A length of more digits than Python reads is longer than any answer
    # that is read, and says so itself: nothing of it is.
    with Raw(endless) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with more than "
        f"{_answer._MOST_ANSWER_BYTES} bytes, more than a QuantForge server sends; is it one?"
    )
    # Zeros before a length are not its digits: the answer is whole, or it
    # is cut short.
    with Raw(padded) as other:
        client = qf.Client(f"http://127.0.0.1:{other.port}")
        client.handshake()
        assert client.proto_version == 1
    with Raw(padded_and_cut_short) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error).endswith(f"IncompleteRead({len(whole)} bytes read, 1 more expected)")
    # Only an answer in chunks has its length elsewhere: beside any other
    # transfer encoding the length stated is the answer's, and what came is
    # held to it, though it reads as an answer of its own.
    with Raw(cut_short_beside_an_encoding_that_is_none) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error).endswith(f"IncompleteRead({len(whole)} bytes read, 1 more expected)")
    # An answer that states no length is read to its end, and to one byte
    # past the most that is read at the longest.
    with Raw(unstated) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    monkeypatch.setattr(_answer, "_MOST_ANSWER_BYTES", 256)
    with Raw(unstated) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert "with more than 256 bytes" in str(error)
    # As long as the longest is taken, and one byte more is not.
    with Raw(closed(b"HTTP/1.0 200 OK\r\n\r\n" + whole.ljust(256))) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    with Raw(closed(b"HTTP/1.0 200 OK\r\n\r\n" + whole.ljust(257))) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert "with more than 256 bytes" in str(error)


def test_an_answer_without_a_body_is_told_by_its_status_whatever_length_it_states():
    # A `304` and a `204` have no body, and may state the length of the one
    # they stand for: that is not a body cut short.
    def not_modified(connection):
        connection.sendall(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 1234\r\n\r\n")

    def no_content(connection):
        connection.sendall(b"HTTP/1.1 204 No Content\r\nContent-Length: 1234\r\n\r\n")

    with Raw(not_modified) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    # No redirect either: it names nowhere to go.
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered 304 Not Modified without "
        "QuantForge's error envelope; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, 304)
    with Raw(no_content) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with a body the "
        "protocol does not read: it is not JSON; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)


def test_an_answer_of_a_stated_length_is_not_waited_for_past_its_end(monkeypatch):
    whole = json.dumps(SPEAKS).encode()

    def kept_open(stated: bytes):
        def serve(connection):
            head = b"HTTP/1.1 200 OK\r\nContent-Length: " + stated + b"\r\n\r\n"
            connection.sendall(head + whole)
            # The server keeps its connection: the client is the one to close.
            try:
                connection.recv(1)
            except OSError:
                pass

        return serve

    # A second and a half would be waited out, were the end of the
    # connection what a read waited for.
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    for stated in (str(len(whole)).encode(), b"0" * 5000 + str(len(whole)).encode()):
        with Raw(kept_open(stated)) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            client.handshake()
            assert (client.server_version, client.proto_version) == ("0.2.0", 1)


def closed(sent: bytes):
    """What a `Raw` does to a connection: it sends this, and closes it."""
    return lambda connection: connection.sendall(sent)


def test_an_answer_without_a_body_is_read_for_none_though_it_says_how_one_would_come(monkeypatch):
    # A `304` may say how the body it stands for would have come, in chunks
    # among other ways, and has none itself. Nothing is read for one: the
    # answer is the refusal its status is, at once, whether the server keeps
    # its connection or closes it.
    chunked = b"\r\nTransfer-Encoding: chunked\r\n\r\n"
    not_modified = b"HTTP/1.1 304 Not Modified" + chunked
    switching = b"HTTP/1.1 101 Switching Protocols" + chunked
    no_content = b"HTTP/1.1 204 No Content" + chunked
    # A second and a half would be waited out, were a body waited for.
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    for serve, said, status in [
        (kept(not_modified), "304 Not Modified", 304),
        (closed(not_modified), "304 Not Modified", 304),
        (kept(switching), "101 Switching Protocols", 101),
    ]:
        with Raw(serve) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        assert str(error) == (
            f"the server at http://127.0.0.1:{other.port}/ answered {said} without QuantForge's "
            "error envelope; is it a QuantForge server?"
        )
        assert (error.code, error.status) == (None, status)
    with Raw(kept(no_content)) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with a body the "
        "protocol does not read: it is not JSON; is it a QuantForge server?"
    )


def test_an_answer_that_only_says_another_is_to_come_is_read_past(monkeypatch):
    whole = json.dumps(SPEAKS).encode()
    answer = b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n%b" % (len(whole), whole)
    early = b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\n"
    going_on = b"HTTP/1.1 100 Continue\r\n\r\n"
    processing = b"HTTP/1.1 102 Processing\r\n\r\n"
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    # A `1xx` is no answer: it says that one is to come, on the same
    # connection, and HTTP asks a client to read past any number of them.
    # `http.client` reads past a `100` alone. Nothing is asked a second
    # time for it.
    for before in (early, going_on, early + processing + early):
        with Raw(kept(before + answer)) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            client.handshake()
            assert (client.server_version, client.proto_version) == ("0.2.0", 1)
        assert len(other.heard) == 1
    # A refusal after one is that refusal.
    with Raw(kept(early + b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert (error.code, error.status) == (None, 404)
    # And one that nothing follows was no answer at all.
    with Raw(closed(early)) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"could not reach the server at http://127.0.0.1:{other.port}/: RemoteDisconnected: "
        "Remote end closed connection without response"
    )


def test_an_answer_in_chunks_is_read_to_its_last_chunk_and_to_the_longest_that_is_taken(
    monkeypatch,
):
    whole = json.dumps(SPEAKS).encode()
    head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"

    def chunk(data: bytes, size: bytes = b"%x", note: bytes = b"") -> bytes:
        return size % len(data) + note + b"\r\n" + data + b"\r\n"

    # In four chunks, their sizes in either case and with zeros before
    # them, one with a note of its own, and with a trailer after the last:
    # the answer ends where its chunks say, and the server, which keeps its
    # connection, is not waited for.
    body = (
        chunk(whole[:1])
        + chunk(whole[1:12], b"%X")
        + chunk(whole[12:38], b"000%x")
        + chunk(whole[38:], note=b" ; note=kept")
    )
    assert b"\r\nB\r\n" in body and b"\r\n0001a\r\n" in body
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    with Raw(kept(head + body + b"0\r\nChecked: yes\r\n\r\n")) as other:
        client = qf.Client(f"http://127.0.0.1:{other.port}")
        client.handshake()
        assert (client.server_version, client.proto_version) == ("0.2.0", 1)
    # As long as the longest is taken. One byte more is not, and is told by
    # the size its chunk states: nothing of that chunk is read.
    monkeypatch.setattr(_answer, "_MOST_ANSWER_BYTES", 256)
    longest = whole.ljust(256)
    with Raw(kept(head + in_chunks(longest[:100], longest[100:]))) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    with Raw(kept(head + in_chunks(longest[:100], longest[100:], b" "))) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with more than 256 "
        "bytes, more than a QuantForge server sends; is it one?"
    )


def test_a_chunk_states_its_size_in_hex_digits_or_the_answer_is_not_http(monkeypatch):
    head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    monkeypatch.setattr(_answer, "_MOST_ANSWER_BYTES", 256)
    # `http.client` takes a chunk's size as Python reads a number: signed,
    # spaced, prefixed. For one below zero it reads to the end of the
    # connection, whatever it was asked for, and for one no read can be
    # asked for it raises what no caller of it expects. Here a size is hex
    # digits and nothing else, told before anything of its chunk is read:
    # the server, which sends more than is taken and keeps its connection,
    # is neither read to its end nor waited for, and what it is refused for
    # does not turn on how its bytes came.
    not_hex = "what is not HTTP (a chunk whose size is not hex digits); is it a QuantForge server?"
    too_long = "more than 256 bytes, more than a QuantForge server sends; is it one?"
    for size, said in [
        (b"-1", not_hex),
        (b"-8" + b"0" * 15, not_hex),
        (b"-" + b"f" * 16, not_hex),
        (b"+5", not_hex),
        (b" 5", not_hex),
        (b"0x5", not_hex),
        (b"5_0", not_hex),
        (b"5g", not_hex),
        (b"", not_hex),
        (b";note=kept", not_hex),
        # A size beyond the longest body that is taken is the answer's own
        # account of being too long, however many digits it is written in.
        (b"101", too_long),
        (b"7" + b"f" * 15, too_long),
        (b"f" * 20, too_long),
        (b"0" * 20 + b"101", too_long),
    ]:
        with Raw(kept(head + size + b"\r\n" + b"x" * 4096)) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        assert str(error) == (
            f"the server at http://127.0.0.1:{other.port}/ answered GET /version with {said}"
        ), size
        assert (error.code, error.status, error.__cause__) == (None, None, None)
    # A chunk is as long as the size it states, and ends there.
    with Raw(kept(head + b"5\r\n{}   and more\r\n0\r\n\r\n")) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
        "HTTP (a chunk that does not end where its size says); is it a QuantForge server?"
    )


def test_an_answer_in_chunks_that_ends_before_its_last_chunk_does_was_cut_short():
    whole = json.dumps(SPEAKS).encode()
    head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
    chunk = in_chunks(whole)[: -len(b"0\r\n\r\n")]
    # Every byte of the answer's JSON came, and the answer did not: it ends
    # with a chunk of no bytes and an empty line after its trailers, and a
    # connection that ends before either may have ended before more of it.
    # `http.client` takes the first three of these for whole.
    for came in [
        chunk,
        chunk + b"0",
        chunk + b"0\r\n",
        chunk + b"0\r\nChecked: yes\r\n",
        chunk + b"0\r\nChecked: y",
        chunk[:-1],
        chunk[:-2],
    ]:
        with Raw(closed(head + came)) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            error = raised(qf.ServerUnavailable, client.handshake)
        cut_short = f"http://127.0.0.1:{other.port}/: IncompleteRead: IncompleteRead("
        assert cut_short in str(error), came
        assert (client.server_version, client.proto_version) == (None, None), came


def test_what_frames_an_answer_is_read_to_a_limit_of_its_own(monkeypatch):
    whole = json.dumps(SPEAKS).encode()
    head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
    assert (_answer._MOST_FRAMING_BYTES, _answer._MOST_LINE_BYTES) == (1024 * 1024, 65536)
    assert _answer._MOST_HEADERS == 100

    def refused_for(sent: bytes) -> str:
        with Raw(kept(sent)) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        assert (error.code, error.status) == (None, None)
        said = f"the server at http://127.0.0.1:{other.port}/ answered GET /version with "
        assert str(error).startswith(said)
        return str(error).removeprefix(said)

    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    # One line of it is as long as `http.client` reads a header's, and an
    # answer has as many headers as it reads: the longest and the most are
    # taken, and one more of either is not.
    length = b"Content-Length: %d\r\n\r\n" % len(whole)

    def with_a_line_of(count: int) -> bytes:
        # The line is `count` bytes, its name and its end among them.
        line = b"X-Long: " + b"a" * (count - len(b"X-Long: \r\n")) + b"\r\n"
        return b"HTTP/1.1 200 OK\r\n" + line + length + whole

    def with_headers(count: int) -> bytes:
        more = b"".join(b"X-%d: 1\r\n" % number for number in range(count - 1))
        return b"HTTP/1.1 200 OK\r\n" + more + length + whole

    for sent in (with_a_line_of(65536), with_headers(100)):
        with Raw(kept(sent)) as other:
            qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    assert refused_for(with_a_line_of(65537)) == (
        "what is not HTTP (a line of more than 65536 bytes); is it a QuantForge server?"
    )
    assert refused_for(with_headers(101)) == (
        "what is not HTTP (more than 100 headers); is it a QuantForge server?"
    )
    # And all of it together is counted, apart from the body: trailers
    # without end, which `http.client` reads for as long as they come, a
    # note on every chunk, answers that only say one is to come, headers.
    # The server sends more than the limit of it and keeps its connection.
    monkeypatch.setattr(_answer, "_MOST_FRAMING_BYTES", 4096)
    too_much = "what is not HTTP (more than 4096 bytes beside its body); is it a QuantForge server?"
    wordy = b"Checked: " + b"y" * 200 + b"\r\n"
    for sent in [
        head + in_chunks(whole)[:-2] + wordy * 1000,
        head + b"".join(b"1;" + b"n" * 500 + b"\r\n \r\n" for _ in range(20)),
        b"HTTP/1.1 100 Continue\r\n\r\n" * 400,
        b"HTTP/1.1 200 OK\r\n" + wordy * 1000,
        b"HTTP/1.1 200 OK\r\nX-Long: " + b"a" * 5000,
    ]:
        assert refused_for(sent) == too_much, sent[:60]
    # Trailers are no more than headers are, however short.
    assert refused_for(head + in_chunks(whole)[:-2] + b"T: 1\r\n" * 101 + b"\r\n") == (
        "what is not HTTP (more than 100 headers); is it a QuantForge server?"
    )
    # A body does not count against it, nor it against the body.
    answer = b"HTTP/1.1 200 OK\r\nContent-Length: 8000\r\n\r\n" + whole.ljust(8000)
    with Raw(kept(answer)) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    with Raw(kept(head + in_chunks(whole.ljust(8000)))) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()


def test_a_body_is_read_in_pieces_of_the_clients_own_size():
    asked = []

    class Recording(io.BytesIO):
        def read1(self, size=-1):
            asked.append(size)
            return super().read1(size)

    def headers(line: bytes):
        return _answer._headers([line])

    piece = _answer._PIECE_BYTES
    long = b"x" * (3 * piece + 1)
    # By the length it states: a piece at a time, and what is left of it at
    # the last. Nothing is asked for past it.
    stated = headers(b"Content-Length: %d" % len(long))
    stream = _answer._Stream(Recording(long + b"more"))
    assert _answer._body(stream, b"HTTP/1.1", 200, stated) == long
    assert asked == [piece, piece, piece, 1]
    # In chunks: each chunk a piece at a time.
    del asked[:]
    chunked = headers(b"Transfer-Encoding: chunked")
    stream = _answer._Stream(Recording(in_chunks(long, b"y")))
    assert _answer._body(stream, b"HTTP/1.1", 200, chunked) == long + b"y"
    assert asked == [piece, piece, piece, 1, 1]
    # To the end of its connection: a piece at a time, to the read that
    # finds nothing.
    del asked[:]
    stream = _answer._Stream(Recording(long))
    assert _answer._body(stream, b"HTTP/1.0", 200, headers(b"X: 1")) == long
    assert asked == [piece] * 5


def test_an_answer_states_one_length_or_it_is_not_http(monkeypatch):
    whole = json.dumps(SPEAKS).encode()
    n, more = str(len(whole)).encode(), str(len(whole) + 1).encode()

    def answer(*headers: bytes, body: bytes = whole) -> bytes:
        lines = b"".join(header + b"\r\n" for header in headers)
        return b"HTTP/1.1 200 OK\r\n" + lines + b"\r\n" + body

    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    # A length said more than once is one length where each says the same,
    # in two headers or in one, and with zeros before it or without. The
    # answer is held to it: read to it and no further, though the server
    # keeps its connection.
    for headers in [
        (b"Content-Length: " + n, b"Content-Length: " + n),
        (b"Content-Length: " + n + b", " + n,),
        (b"Content-Length: " + n, b"Content-Length:\t00" + n + b" "),
    ]:
        with Raw(kept(answer(*headers))) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            client.handshake()
            assert (client.server_version, client.proto_version) == ("0.2.0", 1), headers
    # And cut short where fewer bytes came, though they read as an answer.
    twice = answer(b"Content-Length: " + more, b"Content-Length: " + more)
    with Raw(closed(twice)) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error).endswith(f"IncompleteRead({len(whole)} bytes read, 1 more expected)")
    # Two that differ say nothing of where the answer ends, whichever comes
    # first, and neither does one that is not written as HTTP writes a
    # length, in digits alone.
    differ, no_digits = "two Content-Lengths that differ", "a Content-Length that is not digits"
    for headers, wrong in [
        ((b"Content-Length: 5", b"Content-Length: " + n), differ),
        ((b"Content-Length: " + n, b"Content-Length: 5"), differ),
        ((b"Content-Length: " + n + b", 5",), differ),
        ((b"Content-Length: +" + n,), no_digits),
        ((b"Content-Length: -1",), no_digits),
        ((b"Content-Length: " + n[:1] + b"_" + n[1:],), no_digits),
        ((b"Content-Length: " + n + b",",), no_digits),
        ((b"Content-Length:",), no_digits),
    ]:
        with Raw(kept(answer(*headers))) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            error = raised(qf.RemoteError, client.handshake)
        assert str(error) == (
            f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
            f"HTTP ({wrong}); is it a QuantForge server?"
        ), headers
        assert (error.code, error.status, error.__cause__) == (None, None, None)
        assert (client.server_version, client.proto_version) == (None, None)


def test_an_answer_in_chunks_says_so_in_whatever_case_and_no_other_coding_is_read(monkeypatch):
    whole = json.dumps(SPEAKS).encode()
    more = str(len(whole) + 1).encode()

    def answer(*headers: bytes, version: bytes = b"HTTP/1.1", body: bytes = b"") -> bytes:
        lines = b"".join(header + b"\r\n" for header in headers)
        return version + b" 200 OK\r\n" + lines + b"\r\n" + (body or in_chunks(whole))

    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    # `chunked` in whatever case, and with whatever space around it, which
    # is no part of a header's value: `http.client` knows the one word as
    # it is written here, and hands on the chunks of any other spelling as
    # the body. `identity` is no coding.
    for headers in [
        (b"Transfer-Encoding: chunked",),
        (b"Transfer-Encoding: chunked ",),
        (b"Transfer-Encoding:\tChunked\t",),
        (b"Transfer-Encoding: identity, chunked",),
        (b"Transfer-Encoding: identity", b"Transfer-Encoding: chunked"),
    ]:
        with Raw(kept(answer(*headers))) as other:
            client = qf.Client(f"http://127.0.0.1:{other.port}")
            client.handshake()
            assert (client.server_version, client.proto_version) == ("0.2.0", 1), headers

    def refused_for(sent: bytes) -> str:
        with Raw(kept(sent)) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        said = f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is "
        assert str(error).startswith(said + "not HTTP (")
        assert str(error).endswith("); is it a QuantForge server?")
        wrong = str(error).removeprefix(said + "not HTTP (")
        return wrong.removesuffix("); is it a QuantForge server?")

    # Any other coding is one the client does not read, `chunked` twice
    # among them, and nothing then says where the body ends.
    for headers in [
        (b"Transfer-Encoding: gzip, chunked",),
        (b"Transfer-Encoding: gzip",),
        (b"Transfer-Encoding: chunked, chunked",),
        (b"Transfer-Encoding: chunked", b"Transfer-Encoding: chunked"),
    ]:
        assert refused_for(answer(*headers)) == "a Transfer-Encoding other than chunked", headers
    # An answer says once where its body ends. A length beside its chunks
    # is what no sender may write, whether it is the chunks' length or not,
    # and whether it is a length at all.
    length = b"Content-Length: %d" % len(whole)
    for stated in (length, b"Content-Length: " + more, b"Content-Length: x"):
        headers = (b"Transfer-Encoding: chunked", stated)
        assert refused_for(answer(*headers)) == "a Content-Length beside a Transfer-Encoding"
    # And HTTP/1.0 has no transfer coding: an answer written in it that
    # names one, `identity` among them, is one whose framing is faulty,
    # though its chunks are in order. With a length it is read as before.
    for coding in (b"chunked", b"identity", b""):
        sent = answer(b"Transfer-Encoding: " + coding, version=b"HTTP/1.0")
        assert refused_for(sent) == "a Transfer-Encoding in an answer of HTTP/1.0", coding
    with Raw(kept(answer(length, version=b"HTTP/1.0", body=whole))) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()


def test_a_header_is_a_name_and_a_value_or_the_answer_is_not_http(monkeypatch):
    whole = json.dumps(SPEAKS).encode()
    length = b"Content-Length: %d\r\n" % len(whole)
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)
    # A line that is no header would end the headers for what reads them,
    # and the length after it would go unread: it is refused, and so is a
    # name with nothing in it or a space in it.
    for line in (b"no header at all", b": no name", b"A Name: x", b"Name : x", b" at the start"):
        with Raw(kept(b"HTTP/1.1 200 OK\r\n" + line + b"\r\n" + length + b"\r\n" + whole)) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        assert str(error) == (
            f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
            "HTTP (a header that is not a name and a value); is it a QuantForge server?"
        ), line
    # No line of what frames a body holds a control character but a tab. A
    # carriage return that ends no line ends one for some readers, and what
    # follows it would be a header of its own to them.
    for line in (b"X: a\rContent-Length: 5", b"X: a\x00b", b"X: \x1b[31mred", b"X: a\x7f"):
        with Raw(kept(b"HTTP/1.1 200 OK\r\n" + line + b"\r\n" + length + b"\r\n" + whole)) as other:
            error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
        assert str(error) == (
            f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
            "HTTP (a line with a control character in it); is it a QuantForge server?"
        ), line
    # One that goes on from the line before it is that line's, a tab is
    # space, and a byte that is no ASCII is a value's own business.
    folded = b"X-Note: begins\r\n  and goes on\r\nX-Tab: a\tb\r\nServer: caf\xe9\r\n"
    with Raw(kept(b"HTTP/1.1 200 OK\r\n" + folded + length + b"\r\n" + whole)) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    # Headers that end before their empty line were cut short.
    with Raw(closed(b"HTTP/1.1 200 OK\r\n" + length)) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error).endswith(": IncompleteRead: IncompleteRead(0 bytes read)")


def test_a_refusal_is_told_by_its_status_where_its_body_cannot_be_read(monkeypatch, no_waiting):
    differ = b"Content-Length: 5\r\nContent-Length: 6\r\n\r\nnobody"
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.5)

    def not_http(port: int) -> str:
        return (
            f"the server at http://127.0.0.1:{port}/ answered GET /version with what is not HTTP "
            "(two Content-Lengths that differ); is it a QuantForge server?"
        )

    # A refusal's body is read for the protocol's error envelope and
    # nothing else, so one that does not say where it ends has none: the
    # refusal is what its status says, and why its body was not read is its
    # cause. A proxy's `401` stays the `AuthError` a caller handles.
    with Raw(kept(b"HTTP/1.1 401 Unauthorized\r\n" + differ)) as other:
        error = raised(qf.AuthError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered 401 Unauthorized without "
        "QuantForge's error envelope; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, 401)
    assert type(error.__cause__) is qf.RemoteError and str(error.__cause__) == not_http(other.port)
    # A `503` that names how long to wait is asked again all the same, and
    # is the `ServerUnavailable` it is after the last try.
    with Raw(kept(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 2\r\n" + differ)) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert (error.code, error.status, len(other.heard)) == (None, 503, _wire._ATTEMPTS)
    assert no_waiting == [2] * (_wire._ATTEMPTS - 1)
    assert str(error.__cause__) == not_http(other.port)
    # A success that does not say where it ends is no answer, as before.
    with Raw(kept(b"HTTP/1.1 200 OK\r\n" + differ)) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert (str(error), error.status, error.__cause__) == (not_http(other.port), None, None)
    # And a refusal that was cut short did not come whole, whatever its
    # status: asked again, the server may give it whole, with what it says.
    with Raw(closed(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 100\r\n\r\n{")) as other:
        error = raised(qf.ServerUnavailable, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error).endswith(": IncompleteRead: IncompleteRead(1 bytes read, 99 more expected)")
    assert (error.code, error.status) == (None, None)


@pytest.mark.parametrize(
    "line, wrong",
    [
        (b"HTTP/2 200 OK", "UnknownProtocol"),
        (b"HTTP/1.12 200 OK", "UnknownProtocol"),
        (b"HTTP/1. 200 OK", "UnknownProtocol"),
        (b"HTTP/1.1 20 OK", "BadStatusLine"),
        (b"HTTP/1.1 2000 OK", "BadStatusLine"),
        (b"HTTP/1.1 099 OK", "BadStatusLine"),
        (b"HTTP/1.1 2x0 OK", "BadStatusLine"),
        (b"HTTP/1.1 \xb2\xb0\xb0 OK", "BadStatusLine"),
        (b"HTTP/1.1", "BadStatusLine"),
        (b"ICY 200 OK", "BadStatusLine"),
        (b"", "BadStatusLine"),
    ],
)
def test_a_status_line_is_http_1_and_a_status_of_three_digits_or_the_answer_is_not_http(
    line, wrong
):
    whole = json.dumps(SPEAKS).encode()
    rest = b"\r\nContent-Length: %d\r\n\r\n%b" % (len(whole), whole)
    with Raw(closed(line + rest)) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with what is not "
        f"HTTP ({wrong}); is it a QuantForge server?"
    )
    # And one that is, in each way a server writes it: without a phrase,
    # with tabs for its spaces, of the first revision or a later one.
    for written in (
        b"HTTP/1.1 200",
        b"HTTP/1.1\t200\tFine by me",
        b"HTTP/1.0 200 OK",
        b"HTTP/1.9 200 OK",
    ):
        with Raw(closed(written + rest)) as other:
            qf.Client(f"http://127.0.0.1:{other.port}").handshake()


def test_a_read_that_fails_lets_go_of_its_connection_at_once(monkeypatch):
    let_go = threading.Event()

    def stall(connection):
        # An answer that says it closes its connection, begins, and stops.
        head = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 100\r\n\r\n"
        connection.sendall(head + b"{")
        try:
            connection.recv(1)
        except TimeoutError:
            # Still held, five seconds on.
            return
        except OSError:
            # Let go of, though not by a close this end reads as one.
            pass
        let_go.set()

    monkeypatch.setattr(_wire, "_READ_SECONDS", 0.2)
    with Raw(stall) as stalled:
        client = qf.Client(f"http://127.0.0.1:{stalled.port}")
        error = raised(qf.ServerUnavailable, client.handshake)
        # The error is held here, with every frame it was raised through,
        # the answer's among them: its connection is closed all the same.
        assert let_go.wait(3), "the client holds on to the connection of a read that failed"
    assert str(error).endswith(": TimeoutError: timed out")


# Whatever a server sends: answers put together by a seed, from what a
# QuantForge server sends and from what none does.
HOSTILE_SEED = 0x51A7E5F00D5EED02
HOSTILE_ANSWERS = 1500

STATUS_LINES = [
    *[b"HTTP/1.1 200 OK"] * 6,
    b"HTTP/1.0 200 OK",
    b"HTTP/1.1 304 Not Modified",
    b"HTTP/1.1 204 No Content",
    b"HTTP/1.1 103 Early Hints",
    b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK",
    b"HTTP/1.1 500 Oops",
    b"HTTP/1.1 503 Busy",
    b"HTTP/1.1 301 Moved",
    b"HTTP/1.1 999 Z",
    b"HTTP/1.1 200",
    b"HTTP/2 200 OK",
    b"HTTP/1.1 20 OK",
    b"HTTP/1.1 " + b"9" * 5000 + b" OK",
    b"ICY 200 OK",
    b"",
]
# What a chunk states its size as, in the place of its size.
CHUNK_SIZES = [
    b"-1",
    b"-0",
    b"0",
    b"+5",
    b" 5",
    b"0x5",
    b"5_0",
    b"5;note=kept",
    b"zz",
    b"",
    b"7" + b"f" * 15,
    b"8" + b"0" * 15,
    b"-8" + b"0" * 15,
    b"-" + b"f" * 16,
    b"f" * 20,
    b"-" + b"f" * 20,
    b"1" * 70000,
    b"-" + b"1" * 70000,
]
TRANSFER_ENCODINGS = [
    *[b"chunked"] * 4,
    b"Chunked",
    b"chunked ",
    b"identity",
    b"gzip",
    b"gzip, chunked",
    b"chunked, gzip",
    b"",
]
# How many headers of a kind an answer has.
COUNTS = [0, 1, 1, 1, 1, 2, 3]


def hostile_answer(chosen: random.Random) -> bytes:
    """One answer to `GET /version`, put together from parts `chosen`
    picks: a status line, lengths and transfer encodings that agree with
    the body or do not, a body whole or not, in chunks of the sizes they
    have or of sizes no chunk has, and cut anywhere."""
    whole = json.dumps(SPEAKS).encode()
    n = len(whole)
    lengths = [
        *[b"%d" % n] * 6,
        b"%d" % (n + 1),
        b"%d" % (n - 1),
        b"0",
        b"-1",
        b"+%d" % n,
        b"",
        b"9" * 5000,
        b"0" * 5000 + b"%d" % n,
        b"%d, %d" % (n, n),
        b"%d, 5" % n,
        b"0x10",
        b"%d_" % n,
        b"1e2",
        b"%d" % 2**64,
    ]
    lines = [chosen.choice(STATUS_LINES)]
    lines += [b"Content-Length: " + chosen.choice(lengths) for _ in range(chosen.choice(COUNTS))]
    lines += [
        b"Transfer-Encoding: " + chosen.choice(TRANSFER_ENCODINGS)
        for _ in range(chosen.choice(COUNTS))
    ]
    if chosen.random() < 0.2:
        lines.append(b"Retry-After: " + chosen.choice([b"0", b"9" * 5000, b"-1", b"x", b""]))
    if chosen.random() < 0.05:
        lines.append(b"a line that is no header")
    if chosen.random() < 0.02:
        lines.append(b"X-Long: " + b"a" * 70000)
    if chosen.random() < 0.02:
        lines += [b"X-%d: 1" % number for number in range(150)]
    body = chosen.choice(
        [*[whole] * 6, whole[: n // 2], b"", b"{", whole + b" ", b"[" * 100000, b"\xff\xfe"]
    )
    if chosen.random() < 0.5:
        chunks, left = b"", body
        while left:
            taken = chosen.randint(1, len(left))
            piece, left = left[:taken], left[taken:]
            size = b"%x" % len(piece) if chosen.random() < 0.8 else chosen.choice(CHUNK_SIZES)
            chunks += size + b"\r\n" + piece + chosen.choice([b"\r\n", b"\r\n", b"\n", b""])
        last = [b"0\r\n\r\n", b"0\r\n\r\n", b"0\r\nT: 1\r\n\r\n", b"0\r\n", b""]
        body = chunks + chosen.choice(last)
    answer = b"\r\n".join(lines) + b"\r\n\r\n" + body
    if chosen.random() < 0.1:
        answer = answer[: chosen.randint(0, len(answer))]
    return answer


class Counted(io.BytesIO):
    """An answer as a stream that counts what is read of it, and gives a
    read of its body as many bytes as `chosen` says, one at the least: as a
    connection gives what has come of an answer, and no more."""

    def __init__(self, sent: bytes, chosen: random.Random | None = None):
        super().__init__(sent)
        self.count = 0
        self._chosen = chosen

    def readline(self, size=-1):
        line = super().readline(size)
        self.count += len(line)
        return line

    def read1(self, size=-1):
        if self._chosen is not None and size > 1:
            size = self._chosen.randint(1, size)
        piece = super().read1(size)
        self.count += len(piece)
        return piece


def made_of(sent: bytes, chosen: random.Random | None = None):
    """What the client's reader makes of an answer, its status and its body
    or how it failed, and how many bytes it read for that."""
    stream = Counted(sent, chosen)
    reading = _answer._Stream(stream)
    try:
        version, status, _, headers = _answer._head(reading)
        made = (status, _answer._body(reading, version, status, headers))
    except (_answer._NotHTTP, _answer._TooLong, http.client.HTTPException) as failure:
        made = (type(failure).__name__, str(failure))
    return made, stream.count


def orderly_answer(chosen: random.Random, body: bytes) -> bytes:
    """An answer that holds `body` and is in order, in one of the ways HTTP
    writes one that `chosen` picks."""
    ends = chosen.choice([b"length", b"chunks", b"connection"])
    # HTTP/1.0 has no chunks.
    versions = [b"HTTP/1.1 200 OK", b"HTTP/1.1 200"] + [b"HTTP/1.0 200 OK"] * (ends != b"chunks")
    lines = [chosen.choice(versions)]
    lines += [b"Server: here", b"X-Note: begins\r\n  and goes on"][: chosen.randint(0, 2)]
    if ends == b"length":
        length = b"Content-Length:" + chosen.choice([b" ", b"", b"\t00"]) + b"%d" % len(body)
        lines += [length] * chosen.choice([1, 1, 2])
        body += chosen.choice([b"", b"and what is no part of it"])
    elif ends == b"chunks":
        coding = chosen.choice([b"chunked", b"Chunked ", b"identity, chunked", b"\tCHUNKED"])
        lines += [b"Transfer-Encoding: " + coding]
        chunks, left = b"", body
        while left:
            taken = chosen.randint(1, len(left))
            size = chosen.choice([b"%x", b"%X", b"00%x"]) % taken + chosen.choice([b"", b" ; note"])
            chunks += size + b"\r\n" + left[:taken] + chosen.choice([b"\r\n", b"\n"])
            left = left[taken:]
        trailers = chosen.choice([b"", b"Checked: yes\r\n"])
        body = chunks + b"0\r\n" + trailers + b"\r\n" + chosen.choice([b"", b"and more"])
    to_come = [b"", b"HTTP/1.1 100 Continue\r\n\r\n", b"HTTP/1.1 103 Early Hints\r\n\r\n"]
    before = chosen.choice(to_come) * chosen.randint(1, 3)
    return before + b"\r\n".join(lines) + b"\r\n\r\n" + body


def test_the_reader_of_an_answer_keeps_to_its_limits_however_the_bytes_come(monkeypatch):
    # Limits that the answers here meet.
    monkeypatch.setattr(_answer, "_MOST_ANSWER_BYTES", 64)
    monkeypatch.setattr(_answer, "_MOST_FRAMING_BYTES", 2048)
    monkeypatch.setattr(_answer, "_MOST_LINE_BYTES", 256)
    monkeypatch.setattr(_answer, "_PIECE_BYTES", 16)
    # The most that is read of any answer: its body and what frames it, each
    # to its limit and the one byte past it that tells an answer is past.
    most = (64 + 1) + (2048 + 1)
    chosen = random.Random(HOSTILE_SEED)
    made = collections.Counter()
    for _ in range(4 * HOSTILE_ANSWERS):
        sent = hostile_answer(chosen)
        answer, count = made_of(sent)
        assert count <= most, sent[:200]
        assert isinstance(answer[0], str) or len(answer[1]) <= 64, sent[:200]
        # What is made of an answer does not turn on how its bytes come,
        # down to the words it is refused in.
        for _ in range(2):
            again, count = made_of(sent, chosen)
            assert again == answer, sent[:200]
            assert count <= most, sent[:200]
        made[answer[0]] += 1
    assert {200, "_NotHTTP", "_TooLong", "IncompleteRead", "BadStatusLine"} <= set(made), made
    # And an answer in order is read as it was written, whichever way it
    # says where it ends and however its bytes come.
    for _ in range(4 * HOSTILE_ANSWERS):
        body = bytes(chosen.randrange(256) for _ in range(chosen.choice([0, 1, 20, 63, 64])))
        sent = orderly_answer(chosen, body)
        answer, count = made_of(sent, chosen)
        assert answer == (200, body), sent[:200]
        assert count <= most, sent[:200]


def test_whatever_a_server_sends_a_client_reads_an_answer_or_raises_an_exception_of_its_own(
    monkeypatch, no_waiting
):
    chosen = random.Random(HOSTILE_SEED)
    sent = [b""]
    made = collections.Counter()
    # No answer here is waited for: each is sent whole, and its connection
    # closed. A read that waits all the same fails its answer, not the run.
    monkeypatch.setattr(_wire, "_READ_SECONDS", 5)
    with Raw(lambda connection: connection.sendall(sent[0])) as other:
        url = f"http://127.0.0.1:{other.port}"
        for number in range(HOSTILE_ANSWERS):
            sent[0] = hostile_answer(chosen)
            try:
                qf.Client(url).handshake()
            except qf.RemoteError as refused:
                made[type(refused).__name__] += 1
            except Exception as foreign:
                pytest.fail(
                    f"answer {number} of seed {HOSTILE_SEED:#x} was raised as "
                    f"{type(foreign).__name__}, which is none of the client's: {foreign}; "
                    f"it began {sent[0][:200]!r}"
                )
            else:
                made["read"] += 1
    # Every answer was read, refused as no QuantForge server's, or was one
    # that did not come whole; and each of the three happened.
    assert set(made) == {"read", "RemoteError", "ServerUnavailable"}, made
    assert sum(made.values()) == HOSTILE_ANSWERS


# Jobs, as a server writes them.

STARTED, FINISHED = 1_792_454_401_500, 1_792_454_409_000
FAILURE = {"code": "invalid_request", "message": 'the strategy "x" failed in on_bar: boom'}

# Every state the protocol names, as a server writes it, with what a status
# then gives: when it started, when it ended, and why it failed.
STATES = [
    ({"kind": "queued"}, None, None, None),
    ({"kind": "running", "started_at_ms": STARTED}, STARTED, None, None),
    (
        {"kind": "succeeded", "started_at_ms": STARTED, "finished_at_ms": FINISHED, "results": []},
        STARTED,
        FINISHED,
        None,
    ),
    (
        {"kind": "failed", "started_at_ms": STARTED, "finished_at_ms": FINISHED, "error": FAILURE},
        STARTED,
        FINISHED,
        FAILURE,
    ),
    ({"kind": "failed", "finished_at_ms": FINISHED, "error": FAILURE}, None, FINISHED, FAILURE),
    (
        {"kind": "failed", "started_at_ms": None, "finished_at_ms": FINISHED, "error": FAILURE},
        None,
        FINISHED,
        FAILURE,
    ),
    (
        {"kind": "cancelled", "started_at_ms": STARTED, "finished_at_ms": FINISHED},
        STARTED,
        FINISHED,
        None,
    ),
    ({"kind": "cancelled", "finished_at_ms": FINISHED}, None, FINISHED, None),
]


def job_of(server, status: dict) -> qf.Job:
    """The job a client gets of the stand-in that answers `status`."""
    server.says("GET", f"/v1/backtests/{status['job_id']}", (200, status))
    return qf.Client(server.url).backtest.get(status["job_id"])


@pytest.mark.parametrize("state, started, finished, failure", STATES)
def test_a_status_gives_every_state_of_the_protocol_as_the_server_wrote_it(
    server, state, started, finished, failure
):
    job = job_of(server, written(state=state))
    status = job.status
    assert type(job) is qf.Job and type(status) is qf.JobStatus
    assert (job.id, status.job_id, status.state) == (JOB, JOB, state["kind"])
    assert status.ended == (state["kind"] in ("succeeded", "failed", "cancelled"))
    assert status.submitted_at_ms == 1_792_454_400_000
    assert (status.started_at_ms, status.finished_at_ms) == (started, finished)
    code, message = (failure["code"], failure["message"]) if failure else (None, None)
    assert (status.error_code, status.error_message) == (code, message)
    # What it runs, in the package's own types, every amount exact.
    assert status.bundle == BUNDLE
    assert status.market == qf.MarketId("binance_spot", "BTCUSDT", "1m")
    assert (status.start_open_ms, status.end_open_ms) == (1_792_454_400_000, 1_792_540_740_000)
    assert repr(status.config) == (
        "BacktestConfig(initial_cash=Decimal('10000'), fee_bps=Decimal('7.50'), "
        "close_out_at_end=True)"
    )
    assert repr(status) == f"<JobStatus {JOB} {state['kind']}>"
    assert repr(job) == f"<Job {JOB} {state['kind']}>"


def test_a_state_of_a_later_protocol_is_given_as_the_server_wrote_it(server):
    paused = {"kind": "paused", "since_ms": 5, "started_at_ms": STARTED}
    status = job_of(server, written(state=paused, priority="low")).status
    assert (status.state, status.ended) == ("paused", False)
    assert (status.started_at_ms, status.finished_at_ms) == (STARTED, None)
    assert (status.error_code, status.error_message) == (None, None)
    # One that says when it ended has that read too, and nothing more.
    archived = {"kind": "archived", "finished_at_ms": FINISHED, "error": FAILURE}
    status = job_of(server, written(state=archived)).status
    assert (status.state, status.ended, status.finished_at_ms) == ("archived", False, FINISHED)
    assert (status.error_code, status.error_message) == (None, None)


def test_every_product_line_and_interval_the_engine_has_is_a_jobs_market(server):
    for line in qf.ExchangeId.ALL:
        venue, _, category = str(line).rpartition("_")
        for interval in (qf.Interval.S1, qf.Interval.W1):
            status = written()
            status["submission"]["data"].update(
                exchange=venue, category=category, interval=str(interval)
            )
            assert job_of(server, status).status.market == qf.MarketId(line, "BTCUSDT", interval)


def changed(path: str, value=...) -> dict:
    """A status with the member at `path` changed to `value`, or taken out
    where there is none."""
    status = written(state={"kind": "failed", "finished_at_ms": FINISHED, "error": dict(FAILURE)})
    *parents, last = path.split(".")
    holder = status
    for name in parents:
        holder = holder[name]
    if value is ...:
        del holder[last]
    else:
        holder[last] = value
    return status


UNREADABLE = [
    ([written()], "a job's status is not an object"),
    (changed("job_id"), "a job's status has no `job_id`"),
    (changed("job_id", 7), "the `job_id` of a job's status is not a string"),
    (changed("job_id", "../etc"), "the `job_id` of a job's status is no job ID"),
    (changed("job_id", "j" * 65), "the `job_id` of a job's status is no job ID"),
    (changed("submitted_at_ms"), "a job's status has no `submitted_at_ms`"),
    (
        changed("submitted_at_ms", "1"),
        "the `submitted_at_ms` of a job's status is not an integer",
    ),
    (
        changed("submitted_at_ms", 1.5),
        "the `submitted_at_ms` of a job's status is not an integer",
    ),
    (changed("submission"), "a job's status has no `submission`"),
    (changed("submission", []), "the `submission` of a job's status is not an object"),
    (changed("state"), "a job's status has no `state`"),
    (changed("state", "failed"), "the `state` of a job's status is not an object"),
    (changed("submission.bundle"), f"job {JOB}'s submission has no `bundle`"),
    (
        changed("submission.bundle", BUNDLE.upper()),
        f"the `bundle` of job {JOB}'s submission is no content hash",
    ),
    (
        changed("submission.bundle", BUNDLE[7:]),
        f"the `bundle` of job {JOB}'s submission is no content hash",
    ),
    (
        changed("submission.bundle", BUNDLE + "0"),
        f"the `bundle` of job {JOB}'s submission is no content hash",
    ),
    (changed("submission.data"), f"job {JOB}'s submission has no `data`"),
    (changed("submission.data.category"), f"job {JOB}'s submission has no `category`"),
    (
        changed("submission.data.symbol", 5),
        f"the `symbol` of job {JOB}'s submission is not a string",
    ),
    (
        changed("submission.data.end_open_ms", None),
        f"the `end_open_ms` of job {JOB}'s submission is not an integer",
    ),
    (
        changed("submission.data.exchange", "okx"),
        f"job {JOB}'s submission names the market okx spot BTCUSDT 1m, which this version of the "
        'package does not know (invalid exchange "okx_spot": expected one of binance_spot, '
        "binance_linear, binance_inverse, bybit_spot, bybit_linear, bybit_inverse)",
    ),
    (
        changed("submission.data.interval", "7m"),
        f"job {JOB}'s submission names the market binance spot BTCUSDT 7m, which this version of "
        "the package does not know (invalid interval: 7m)",
    ),
    (
        changed("submission.data.symbol", "btcusdt"),
        f"job {JOB}'s submission names the market binance spot btcusdt 1m, which is not in its "
        "canonical form",
    ),
    (
        changed("submission.data.interval", " 1m"),
        f"job {JOB}'s submission names the market binance spot BTCUSDT  1m, which is not in its "
        "canonical form",
    ),
    (changed("submission.backtest"), f"job {JOB}'s submission has no `backtest`"),
    (
        changed("submission.backtest.initial_cash", 10000),
        f"the `initial_cash` of job {JOB}'s submission is not a string",
    ),
    (
        changed("submission.backtest.fee_bps", "1e1"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "07.5"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "+7.5"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "7."),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "٧"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "-0"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "-0.00"),
        f"the `fee_bps` of job {JOB}'s submission is no decimal in its exact form",
    ),
    (
        changed("submission.backtest.fee_bps", "-0.5"),
        f"job {JOB}'s submission has settings the engine refuses (invalid configuration: "
        "fee_bps must be zero or greater, got -0.5)",
    ),
    (
        changed("submission.backtest.initial_cash", "0"),
        f"job {JOB}'s submission has settings the engine refuses (invalid configuration: "
        "initial_cash must be greater than 0, got 0)",
    ),
    (
        changed("submission.backtest.close_out_at_end", 1),
        f"the `close_out_at_end` of job {JOB}'s submission is not true or false",
    ),
    (changed("state.kind"), f"job {JOB}'s state has no `kind`"),
    (changed("state.kind", 3), f"the `kind` of job {JOB}'s state is not a string"),
    (changed("state.finished_at_ms"), f"job {JOB}'s state has no `finished_at_ms`"),
    (changed("state.error"), f"job {JOB}'s state has no `error`"),
    (changed("state.error.code"), f"job {JOB}'s error has no `code`"),
    (changed("state.error.message", None), f"the `message` of job {JOB}'s error is not a string"),
    (
        changed("state", {"kind": "running"}),
        f"job {JOB}'s state has no `started_at_ms`",
    ),
    (
        changed("state", {"kind": "running", "started_at_ms": None}),
        f"the `started_at_ms` of job {JOB}'s state is not an integer",
    ),
    (
        changed("state", {"kind": "succeeded", "started_at_ms": 1, "finished_at_ms": 2}),
        f"job {JOB}'s state has no `results`",
    ),
    (
        changed("state", {"kind": "succeeded", "finished_at_ms": 2, "results": []}),
        f"job {JOB}'s state has no `started_at_ms`",
    ),
    (
        changed("state", {"kind": "cancelled", "started_at_ms": True, "finished_at_ms": 2}),
        f"the `started_at_ms` of job {JOB}'s state is not an integer",
    ),
]


@pytest.mark.parametrize("answer, why", UNREADABLE)
def test_a_status_the_protocol_does_not_write_is_refused_and_never_guessed_at(server, answer, why):
    client = qf.Client(server.url)
    server.says("GET", f"/v1/backtests/{JOB}", (200, answer))
    error = raised(qf.RemoteError, lambda: client.backtest.get(JOB))
    assert str(error) == (
        f"the server at {server.url}/ answered GET /v1/backtests/{JOB} with a body the protocol "
        f"does not read: {why}; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)
    # One such job among those listed refuses the list.
    server.says("GET", "/v1/backtests", (200, {"jobs": [written(job_id="0" * 32), answer]}))
    assert str(raised(qf.RemoteError, client.backtest.list)).endswith(
        f"does not read: {why}; is it a QuantForge server?"
    )


@pytest.mark.parametrize(
    "answer, why",
    [
        ("[]", "the list of jobs is not an object"),
        ("{}", "the list of jobs has no `jobs`"),
        ('{"jobs":{}}', "the `jobs` of the list of jobs is not a list"),
        ('{"jobs":[7]}', "a job's status is not an object"),
        ("not json", "it is not JSON"),
    ],
)
def test_a_list_the_protocol_does_not_write_is_refused(server, answer, why):
    server.says("GET", "/v1/backtests", (200, answer))
    error = raised(qf.RemoteError, qf.Client(server.url).backtest.list)
    assert str(error) == (
        f"the server at {server.url}/ answered GET /v1/backtests with a body the protocol does "
        f"not read: {why}; is it a QuantForge server?"
    )


def test_an_amount_is_in_the_protocols_exact_form_or_it_is_none(server, repository):
    # What the protocol's own tests refuse as no exact decimal, read from
    # the crate: a minus sign on a zero among it.
    source = (repository / "src" / "protocol" / "decimal.rs").read_text(encoding="utf-8")
    (listed,) = re.findall(
        r"fn anything_but_the_exact_form_is_refused\(\) \{\s+for text in \[(.*?)\] \{", source, re.S
    )
    refused = re.findall(r'"([^"]*)"', listed)
    assert len(refused) > 20 and {"-0", "-0.00", "+1", "1e5", ""} <= set(refused)
    assert [text for text in refused if _jobs._is_exact_decimal(text)] == []
    # And what it takes, each read with the digits it was written with.
    for text in ["0", "0.00", "7.50", "10", "0.0001", "81940.01000000"]:
        assert _jobs._is_exact_decimal(text) and _jobs._is_exact_decimal(f"-{text}") == (
            text.strip("0.") != ""
        ), text
        status = written()
        status["submission"]["backtest"]["fee_bps"] = text
        assert format(job_of(server, status).status.config.fee_bps, "f") == text


def test_a_job_is_given_only_the_status_of_the_job_it_is(server):
    other = "0" * 32
    said = f"it is the status of job {other}, not of job {JOB}"
    client = qf.Client(server.url)
    server.says("GET", f"/v1/backtests/{JOB}", (200, written(job_id=other)))
    assert said in str(raised(qf.RemoteError, lambda: client.backtest.get(JOB)))
    running = written(state={"kind": "running", "started_at_ms": STARTED})
    server.says("GET", f"/v1/backtests/{JOB}", (200, running), (200, written(job_id=other)))
    server.says("POST", f"/v1/backtests/{JOB}/cancel", (200, written(job_id=other)))
    job = client.backtest.get(JOB)
    for ask in (job.refresh, job.cancel):
        assert said in str(raised(qf.RemoteError, ask))
        # What it was last given stays.
        assert (job.id, job.status.state) == (JOB, "running")


def test_a_list_is_the_servers_jobs_in_the_servers_order_each_a_job_of_its_own(server):
    ids = ["c" * 32, "a" * 32, "b" * 32]
    server.says("GET", "/v1/backtests", (200, {"jobs": [written(job_id=each) for each in ids]}))
    server.says("GET", "/v1/backtests?limit=2", (200, {"jobs": [written(job_id=ids[0])]}))
    client = qf.Client(server.url)
    listed = client.backtest.list()
    assert type(listed) is list and [job.id for job in listed] == ids
    assert all(type(job) is qf.Job for job in listed)
    assert [job.id for job in client.backtest.list(limit=2)] == ids[:1]
    assert client.backtest.list(limit=None) is not listed
    assert server.routes[1:] == [
        "GET /v1/backtests",
        "GET /v1/backtests?limit=2",
        "GET /v1/backtests",
    ]


@pytest.mark.parametrize("limit", ["3", 3.0, True, [3], b"3"])
def test_a_limit_is_a_whole_number_and_the_server_says_which(server, limit, refused):
    client = qf.Client(server.url)
    said = f"limit must be an int or None, not {type(limit).__name__}"
    refused(TypeError, said, client.backtest.list, limit=limit)
    # And is given by its name.
    with pytest.raises(TypeError, match="takes 1 positional argument but 2 were given"):
        client.backtest.list(3)
    # Nothing was asked for it, not so much as the server's versions.
    assert server.asked == []


@pytest.mark.parametrize(
    "job_id, shown",
    [
        ("", "''"),
        ("a b", "'a b'"),
        ("../v1", "'../v1'"),
        ("a/b", "'a/b'"),
        ("jöb", "'jöb'"),
        ("a\n", "'a\\n'"),
        ("j" * 65, "'" + "j" * 65 + "'"),
        ("?" * 200, "'" + "?" * 80 + "'..."),
    ],
)
def test_what_is_no_job_id_is_refused_before_anything_is_asked(server, job_id, shown, refused):
    client = qf.Client(server.url)
    said = f"invalid job id {shown}: expected 1 to 64 ASCII letters, digits, '-', or '_'"
    refused(ValueError, said, client.backtest.get, job_id)
    for given in (7, None, b"abc"):
        said = f"job_id must be a str, not {type(given).__name__}"
        refused(TypeError, said, client.backtest.get, given)
    assert server.asked == []


def test_refresh_and_cancel_ask_the_server_and_keep_what_it_answers(server):
    running = written(state={"kind": "running", "started_at_ms": STARTED})
    ended = {"kind": "cancelled", "started_at_ms": STARTED, "finished_at_ms": FINISHED}
    job = job_of(server, running)
    before = job.status
    del server.asked[:]
    # Reading a job asks nothing.
    assert (job.id, job.status.state, job.status.ended, repr(job)) == (
        JOB,
        "running",
        False,
        f"<Job {JOB} running>",
    )
    assert server.asked == []
    # `refresh` gives the status it was answered, and keeps it.
    server.says("GET", f"/v1/backtests/{JOB}", (200, running))
    again = job.refresh()
    assert again is job.status and again == before and again is not before
    # `cancel` is a POST without a body, and so does it.
    server.says("POST", f"/v1/backtests/{JOB}/cancel", (200, written(state=ended)))
    cancelled = job.cancel()
    assert cancelled is job.status and cancelled.state == "cancelled" and cancelled.ended
    assert (cancelled.started_at_ms, cancelled.finished_at_ms) == (STARTED, FINISHED)
    assert server.routes == [f"GET /v1/backtests/{JOB}", f"POST /v1/backtests/{JOB}/cancel"]
    method, _, headers, sent = server.asked[-1]
    assert (method, headers["Content-Length"], sent) == ("POST", "0", b"")
    # A status that was read is a value: the old one says what it said.
    assert (before.state, before.finished_at_ms, before != cancelled) == ("running", None, True)
    # A refusal leaves the job as it was last given.
    server.says("GET", f"/v1/backtests/{JOB}", refusal(404, "not_found", "the server holds no job"))
    raised(qf.NotFound, job.refresh)
    assert job.status is cancelled


def test_a_cancel_is_answered_cancelled_or_it_is_no_answer(server):
    running = written(state={"kind": "running", "started_at_ms": STARTED})
    job = job_of(server, running)
    cancel = f"/v1/backtests/{JOB}/cancel"
    answered = 0
    for state, *_ in STATES:
        server.says("POST", cancel, (200, written(state=state)))
        if state["kind"] == "cancelled":
            # Before it started, and after.
            assert job.cancel().state == "cancelled"
            answered += 1
            continue
        # A cancel that is answered with the job in another state is one the
        # protocol does not write, and says nothing of the job.
        before = job.status
        error = raised(qf.RemoteError, job.cancel)
        assert str(error) == (
            f"the server at {server.url}/ answered POST {cancel} with a body the protocol does "
            f"not read: it says that job {JOB} is {state['kind']}, and a job that was cancelled "
            "is cancelled; is it a QuantForge server?"
        )
        assert job.status is before
    assert answered == 2
    # A state a later revision of the protocol answers a cancel with is
    # given as the server wrote it, as anywhere.
    later = {"kind": "cancelling", "started_at_ms": STARTED}
    server.says("POST", cancel, (200, written(state=later)))
    assert (job.cancel().state, job.status.ended) == ("cancelling", False)
    # And a status that is asked for is given in any state.
    for state, *_ in STATES:
        server.says("GET", f"/v1/backtests/{JOB}", (200, written(state=state)))
        assert job.refresh().state == state["kind"]


# What the classes are.


def test_a_status_is_a_value_equal_by_its_fields_and_no_key(server):
    states = [state for state, *_ in STATES]
    first = [job_of(server, written(state=state)).status for state in states]
    second = [job_of(server, written(state=state)).status for state in states]
    # An absent start and a `null` one are the same status, and no two
    # others are.
    assert states[4] != states[5] and first[4] == first[5]
    alike = {4: [4, 5], 5: [4, 5]}
    for at, status in enumerate(first):
        assert status == second[at] and not status != second[at]
        equal = [other for other, each in enumerate(first) if status == each]
        assert equal == alike.get(at, [at]), at
    # Each field counts, an amount by its value.
    base = first[0]
    for path, value in [
        ("job_id", "0" * 32),
        ("submitted_at_ms", 1),
        ("submission.bundle", BUNDLE[:-1] + "0"),
        ("submission.data.symbol", "ETHUSDT"),
        ("submission.data.start_open_ms", 0),
        ("submission.data.end_open_ms", 0),
        ("submission.backtest.fee_bps", "7.51"),
        ("submission.backtest.close_out_at_end", False),
    ]:
        other = changed(path, value)
        other["state"] = {"kind": "queued"}
        assert job_of(server, other).status != base, path
    same = changed("submission.backtest.fee_bps", "7.5")
    same["state"] = {"kind": "queued"}
    assert job_of(server, same).status == base
    assert base != JOB and base != written() and (base == object()) is False
    with pytest.raises(TypeError, match="unhashable type: 'JobStatus'"):
        hash(base)
    # A job is a handle, and equal to itself alone.
    one, other = job_of(server, written()), job_of(server, written())
    assert one == one and one != other and one.status == other.status
    assert len({one, other}) == 2


def test_no_class_of_the_client_is_built_derived_from_changed_or_pickled(server, refused):
    client = qf.Client(server.url, token=TOKEN)
    job = job_of(server, written())
    refused(
        TypeError,
        "a Job cannot be built: Client.backtest gives one, by get(job_id) or in list()",
        qf.Job,
    )
    refused(
        TypeError,
        "a JobStatus cannot be built: it is what a server answered of a job",
        qf.JobStatus,
        job_id=JOB,
    )
    for value, held, missing in [
        (client, ["url", "backtest", "server_version", "proto_version", "handshake"], "token"),
        (job, ["id", "status", "refresh", "cancel"], "state"),
        (job.status, ["job_id", "state", "ended", "market", "config", "error_message"], "id"),
    ]:
        named = f"quantforge_py.{type(value).__name__}"
        for name in held:
            said = f"attribute '{name}' of '{named}' objects is not writable"
            refused(AttributeError, said, setattr, value, name, None)
            refused(AttributeError, said, delattr, value, name)
        said = f"'{named}' object has no attribute '{missing}'"
        refused(AttributeError, said, setattr, value, missing, None)
        assert not hasattr(value, "__dict__")
        said = f"cannot pickle '{named}' object"
        for copied in (pickle.dumps, copy.copy, copy.deepcopy):
            refused(TypeError, said, copied, value)
        said = f"type '{named}' is not an acceptable base type"
        refused(TypeError, said, type, "Derived", (type(value),), {})
    # A token is nowhere a client can be asked for it.
    assert not any(TOKEN in repr(getattr(client, name)) for name in dir(client) if name[:2] != "__")


ERRORS = [
    qf.AuthError,
    qf.ValidationError,
    qf.NotFound,
    qf.Conflict,
    qf.ServerUnavailable,
    qf.VersionMismatch,
]


def test_every_failure_of_a_request_is_a_remote_error_and_an_exception_like_any_other():
    assert qf.RemoteError.__bases__ == (Exception,)
    for kind in ERRORS:
        assert kind.__bases__ == (qf.RemoteError,), kind
        assert not any(issubclass(kind, other) for other in ERRORS if other is not kind), kind
    for kind in [qf.RemoteError, *ERRORS]:
        error = kind("the server said so", code="some_code", status=418)
        assert (str(error), error.args) == ("the server said so", ("the server said so",))
        assert repr(error) == f"{kind.__name__}('the server said so')"
        assert (error.code, error.status) == ("some_code", 418)
        assert (kind("m").code, kind("m").status) == (None, None)
        # One crosses to another process as itself: a pool of processes
        # raises in its parent what a child was raised.
        for copied in (pickle.loads(pickle.dumps(error)), copy.copy(error)):
            assert type(copied) is kind and str(copied) == "the server said so"
            assert (copied.code, copied.status) == ("some_code", 418)
        # And is derived from, as any exception is.
        assert issubclass(type("Derived", (kind,), {}), kind)
    mismatch = qf.VersionMismatch("m", supported=(2, 3))
    assert qf.VersionMismatch("m").supported is None
    assert pickle.loads(pickle.dumps(mismatch)).supported == (2, 3)


def test_importing_the_package_imports_nothing_of_the_network(server):
    # What a request takes is imported when a client first makes one: the
    # package imports as fast with a client in it as without. The client of
    # this fresh interpreter asks the stand-in, so nothing here depends on
    # what listens on the machine, or does not.
    code = """
        import sys

        import quantforge_py as qf

        def loaded(*names):
            return sorted(name for name in names if name in sys.modules)

        print(loaded("http", "http.client", "ssl", "socket", "json", "email", "urllib.parse"))
        client = qf.Client(sys.argv[1], token="a-token")
        client.backtest, client.url, repr(client)
        print(loaded("http.client", "ssl", "socket", "json", "email"))
        client.handshake()
        print(loaded("http.client", "socket", "json"), client.proto_version)
    """
    run = subprocess.run(
        [sys.executable, "-c", textwrap.dedent(code), server.url],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["[]", "[]", "['http.client', 'json', 'socket'] 1"]
    assert server.routes == ["GET /version"] and authorization(server) == ["Bearer a-token"]
