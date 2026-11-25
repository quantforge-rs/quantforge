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

import concurrent.futures
import copy
import http.server
import json
import pickle
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
from quantforge_py import _errors, _jobs, _wire

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
    # system's resolver says. Here it leads to this machine, and the token
    # goes.
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
    if sys.get_int_max_str_digits():
        # Python refuses to read a number so long, which is why none is.
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
    assert _wire._MOST_ANSWER_BYTES == 64 * 1024 * 1024
    monkeypatch.setattr(_wire, "_MOST_ANSWER_BYTES", 256)
    client = qf.Client(server.url)
    client.handshake()
    for status in (200, 500):
        server.says("GET", "/v1/backtests", (status, "x" * 257))
        error = raised(qf.RemoteError, client.backtest.list)
        assert str(error) == (
            f"the server at {server.url}/ answered GET /v1/backtests with more than 256 bytes, "
            "more than a QuantForge server sends; is it one?"
        )
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

    # A length of more digits than Python reads is longer than any answer
    # that is read, and says so itself: nothing of it is.
    with Raw(endless) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert str(error) == (
        f"the server at http://127.0.0.1:{other.port}/ answered GET /version with more than "
        f"{_wire._MOST_ANSWER_BYTES} bytes, more than a QuantForge server sends; is it one?"
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
    # An answer that states no length is read to its end, and to one byte
    # past the most that is read at the longest.
    with Raw(unstated) as other:
        qf.Client(f"http://127.0.0.1:{other.port}").handshake()
    monkeypatch.setattr(_wire, "_MOST_ANSWER_BYTES", 256)
    with Raw(unstated) as other:
        error = raised(qf.RemoteError, qf.Client(f"http://127.0.0.1:{other.port}").handshake)
    assert "with more than 256 bytes" in str(error)


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
