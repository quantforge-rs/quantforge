"""A QuantForge server as a client's requests reach it: where it is, which
token goes with a request, each request on a connection of its own, what is
asked again, an answer read strictly, and a refusal raised as the exception
its code names (`docs/python-client.md`).

It speaks the protocol the server's routes are specified in
(`quantforge::protocol`, the crate's own documentation), with the standard
library alone:

- **A protocol version is agreed on first,** through `GET /version`, when a
  client is first used and never before, and every later request is asked
  under that version's paths.
- **A refusal is the server's own,** word for word: the protocol's error
  envelope, raised as the exception its code names.
- **A token is sent with every request,** as `Authorization: Bearer`, and
  never shown. It goes only over `https`, or over `http` to this machine.
- **Every connection goes to the server itself.** `http.client` reads no
  proxy from the environment and follows no redirect, so nothing here does
  either.
- **An answer is read by the client itself,** in `quantforge_py._answer`,
  and not by `http.client`, which opens the connection and writes the
  request: an answer says where it ends as HTTP writes that, or it is
  refused, and its body and what frames it are each read to a limit.
- **Every request made here may be sent again,** and is, a few times, while
  the server answers `503` with `Retry-After`: a read changes nothing, and a
  cancel nothing more the second time. A submission, which a later story
  adds, is never sent twice: one the server took but could not answer for
  would run twice.

Importing this module imports nothing of the network. `http.client`, `ssl`
and what else a request takes are imported when a client first needs them,
so the package imports as fast with a client in it as without.

This module is private.
"""

import os
import time

from . import _answer
from ._errors import _RAISED, _RAISED_BY_STATUS, RemoteError, ServerUnavailable, VersionMismatch
from ._native import __version__

# The variable a token is read from where the argument is left out: the
# command line's, for `--remote-token`.
_TOKEN_VARIABLE = "QF_REMOTE_TOKEN"

# The protocol versions a client speaks, the oldest and the newest: the
# engine's `MIN_SUPPORTED` and `PROTO_VERSION`, which the tests hold them to.
_MIN_SUPPORTED = 1
_PROTO_VERSION = 1

# How long opening a connection may take, TLS included, and how long the
# server may stay silent while it answers, in seconds.
_CONNECT_SECONDS = 10
_READ_SECONDS = 60

# How many times a request is sent while the server answers `503` with
# `Retry-After`, and the longest one such wait, in seconds.
_ATTEMPTS = 5
_MOST_RETRY_SECONDS = 5

# How much of a value that is refused is quoted.
_QUOTED = 80

_EXAMPLE = "http://127.0.0.1:9600"

# What a path of a server's URL is written with as it is.
_PATH_SAFE = "/%-._~!$&'()*+,;=:@"

# The integers the protocol's own types hold.
_MOST_INTEGER = 2**63


class _Unreadable(Exception):
    """An answer that is not what the protocol says its route answers."""


def _wrong_type(what, takes, value):
    """A value of a type `what` does not take, worded as the extension words
    one."""
    return TypeError(f"{what} must be {takes}, not {type(value).__name__}")


def _quoted(text):
    """A value that is refused, as Python writes it, and in part where it is
    very long."""
    if len(text) > _QUOTED:
        return f"{text[:_QUOTED]!r}..."
    return repr(text)


def _tls_context():
    """How a server's certificate is checked over `https`: by Python's own
    defaults, against what the system trusts and the name the URL gives."""
    import ssl

    return ssl.create_default_context()


def _loopback(address):
    """Whether text is a loopback address, which is this machine's: of IPv4,
    of IPv6, or of IPv4 as IPv6 writes one."""
    import ipaddress

    try:
        parsed = ipaddress.ip_address(address)
    except ValueError:
        return False
    return (getattr(parsed, "ipv4_mapped", None) or parsed).is_loopback


def _this_machine(host):
    """Whether a URL's host names this machine: a loopback address, or
    `localhost`, which is held to leading to one as a connection is made."""
    return host.lower() == "localhost" or _loopback(host)


class _Server:
    """Where a server is, as `base_url` names it: checked, and written
    without anything a URL may not show."""

    __slots__ = ("https", "host", "port", "path", "shown", "local")

    def __init__(self, base_url):
        import urllib.parse

        if not isinstance(base_url, str):
            raise _wrong_type("base_url", "a str", base_url)
        text = base_url.strip()
        if not text:
            raise ValueError(f"base_url is empty; name the server, such as {_EXAMPLE}")
        if any(char.isspace() or not char.isprintable() for char in text):
            raise ValueError(
                "base_url holds a space or a character that is not printed; name the server, "
                f"such as {_EXAMPLE}"
            )
        if "?" in text or "#" in text:
            raise ValueError(
                "base_url holds a query or a fragment; name the server and the path it is served "
                "under, and nothing more"
            )
        # A URL can carry a password, so no refusal here quotes it, and none
        # is raised with what the standard library says of it behind it.
        try:
            parts = urllib.parse.urlsplit(text)
            host, port = parts.hostname, parts.port
            carries = parts.username is not None or parts.password is not None
        except ValueError:
            parts = None
        if parts is None:
            raise ValueError(
                f"base_url is not a URL the client reads; name the server, such as {_EXAMPLE}"
            )
        if parts.scheme not in ("http", "https") or not text[len(parts.scheme) :].startswith("://"):
            raise ValueError(f"base_url must start with http:// or https://, such as {_EXAMPLE}")
        if carries:
            raise ValueError(
                "base_url holds a user name or a password, which the client does not send; give "
                f"the server's token as token, or in {_TOKEN_VARIABLE}"
            )
        if not host:
            raise ValueError(f"base_url names no host; name the server, such as {_EXAMPLE}")
        if port == 0:
            raise ValueError("base_url names port 0, which no server listens on")
        self.https = parts.scheme == "https"
        usual = 443 if self.https else 80
        self.host = host
        self.port = usual if port is None else port
        path = parts.path if parts.path.endswith("/") else f"{parts.path}/"
        self.path = urllib.parse.quote(path, safe=_PATH_SAFE)
        named = f"[{host}]" if ":" in host else host
        if self.port != usual:
            named = f"{named}:{self.port}"
        self.shown = f"{parts.scheme}://{named}{self.path}"
        self.local = _this_machine(host)


def _token(given):
    """The token to send and where it came from, the argument or the
    variable, or `(None, None)` where there is none. An empty one counts as
    none, and an argument that is given never falls back to the variable."""
    if given is None:
        text, source = os.environ.get(_TOKEN_VARIABLE), _TOKEN_VARIABLE
        if text is None:
            return None, None
    elif isinstance(given, str):
        text, source = given, "token"
    else:
        raise _wrong_type("token", "a str or None", given)
    text = text.strip()
    if not text:
        return None, None
    # What a header's value may hold, so that nothing below ever has a reason
    # to refuse the header, which it would do by quoting it.
    if not all("!" <= char <= "~" for char in text):
        raise ValueError(
            f"{source} holds a character other than printable ASCII, or a space inside it: a "
            "token is sent in a header"
        )
    return text, source


class _Answer:
    """One answer of a server: its status, how long it asks to wait, its
    body, how many refusals of a version the client had counted as the
    request it answers was sent, and, for a refusal whose body could not be
    read, why not."""

    __slots__ = ("status", "reason", "retry_after", "body", "forgotten", "unread")

    def __init__(self, status, reason, retry_after, body, forgotten, unread=None):
        self.status = status
        self.reason = reason
        self.retry_after = retry_after
        self.body = body
        self.forgotten = forgotten
        self.unread = unread

    @property
    def said(self):
        """The status with its phrase, such as `404 Not Found`."""
        import http.client

        phrase = http.client.responses.get(self.status)
        if phrase is None:
            # The server's own, for a status that has none: as long as it
            # is a few printable words, which is all a message has room for.
            said = self.reason
            phrase = said if len(said) <= _QUOTED and said.isascii() and said.isprintable() else ""
        return f"{self.status} {phrase}".strip()


def _retry_after(text):
    """How long a `503`'s `Retry-After` asks to wait, in seconds, at most
    `_MOST_RETRY_SECONDS`; `None` without one, which says not to ask
    again."""
    if text is None:
        return None
    text = text.strip()
    if not (text.isascii() and text.isdigit()):
        return None
    return _answer._at_most(text, _MOST_RETRY_SECONDS)


def _parsed(body):
    """An answer's body as JSON, read strictly: UTF-8, and no number that
    JSON does not have. A body that is none is an `_Unreadable`."""
    import json

    def refuse(constant):
        raise ValueError(f"{constant} is no JSON number")

    try:
        return json.loads(body.decode("utf-8"), parse_constant=refuse)
    except (ValueError, RecursionError):
        raise _Unreadable("it is not JSON") from None


_JSON = {
    str: "a string",
    int: "an integer",
    bool: "true or false",
    dict: "an object",
    list: "a list",
}


def _object(value, what):
    if not isinstance(value, dict):
        raise _Unreadable(f"{what} is not an object")
    return value


def _member(record, name, kind, what):
    """A member the protocol requires of an answer's object, of the JSON
    type it gives it."""
    if name not in record:
        raise _Unreadable(f"{what} has no `{name}`")
    value = record[name]
    # A JSON `true` is no number, though Python counts it among its ints.
    if not isinstance(value, kind) or (kind is int and isinstance(value, bool)):
        raise _Unreadable(f"the `{name}` of {what} is not {_JSON[kind]}")
    if kind is int and not -_MOST_INTEGER <= value < _MOST_INTEGER:
        raise _Unreadable(f"the `{name}` of {what} is beyond the integers the protocol holds")
    return value


def _optional(record, name, kind, what):
    """A member the protocol leaves out when it has no value: a missing one
    and a `null` are read alike."""
    if record.get(name) is None:
        return None
    return _member(record, name, kind, what)


def _versions(record, what):
    """The protocol versions one side speaks, the oldest and the newest: a
    range that holds a version."""
    oldest = _member(record, "min_supported", int, what)
    newest = _member(record, "proto_version", int, what)
    if not 1 <= oldest <= newest:
        raise _Unreadable(f"{what} names the versions {oldest} to {newest}, which hold none")
    return oldest, newest


def _envelope(body):
    """The protocol's error envelope in an answer's body, as its code, its
    message and the versions it lists; `None` where the body is none."""
    try:
        error = _member(_object(_parsed(body), "the body"), "error", dict, "the body")
        code = _member(error, "code", str, "the error")
        message = _member(error, "message", str, "the error")
        listed = _optional(error, "supported", dict, "the error")
        supported = None if listed is None else _versions(listed, "the error's supported")
    except _Unreadable:
        return None
    return code, message, supported


class _Wire:
    """One server as requests reach it: each sent on a connection of its
    own, with the token, and answered, or failed as the exception a caller
    is raised."""

    def __init__(self, server, token):
        import threading

        self.server = server
        self._headers = {
            "Accept": "application/json",
            "Connection": "close",
            "User-Agent": f"quantforge-py/{__version__}",
        }
        if token is not None:
            self._headers["Authorization"] = f"Bearer {token}"
        # A token over plain `http` goes to this machine alone, which a
        # connection is held to as it is made: a name leads where the
        # system's resolver says.
        self._local_only = token is not None and not server.https
        self._context = None
        # The server's release and the protocol version agreed with it, once
        # the server has been asked; and how many times a refusal has had it
        # forgotten, so that an answer that was on its way meanwhile is not
        # taken for the server's latest.
        self.agreed = None
        self._forgotten = 0
        self._lock = threading.Lock()

    def _connection(self):
        import http.client

        server = self.server
        if not server.https:
            return http.client.HTTPConnection(server.host, server.port, timeout=_CONNECT_SECONDS)
        # Made when it is first needed, since reading what the system trusts
        # takes a moment, and kept. Two threads that need it at the same time
        # each make one, and either serves.
        if self._context is None:
            self._context = _tls_context()
        return http.client.HTTPSConnection(
            server.host, server.port, timeout=_CONNECT_SECONDS, context=self._context
        )

    def _held_to_this_machine(self, connection):
        """Refuses a connection that does not end on this machine, before
        anything is sent on it."""
        peer = connection.sock.getpeername()[0]
        if not _loopback(peer):
            raise RemoteError(
                f"the server at {self.server.shown} is not on this machine: its name leads to "
                f"{peer} here, and a token goes over http to this machine alone; nothing was sent"
            )

    def _not_http(self, method, target, wrong):
        """The exception an answer that is not HTTP is raised as: `wrong` is
        what was wrong with it."""
        return RemoteError(
            f"the server at {self.server.shown} answered {method} {target} with what is not "
            f"HTTP ({wrong}); is it a QuantForge server?"
        )

    def _too_long(self, method, target):
        """The exception an answer with a body longer than is taken is
        raised as."""
        return RemoteError(
            f"the server at {self.server.shown} answered {method} {target} with more than "
            f"{_answer._MOST_ANSWER_BYTES} bytes, more than a QuantForge server sends; is it one?"
        )

    def _read(self, stream, method, target, forgotten):
        """One answer, read from its connection: its status and its headers,
        which have to be whole, and then its body.

        A success whose body cannot be read is no answer. A refusal is told
        by its status, and its body is read for the protocol's error
        envelope and nothing else: where that body is longer than is taken,
        or does not say where it ends, the refusal has no envelope, and why
        its body was not read is kept, to be its cause. A body that was cut
        short is another matter, and is raised as it is: the answer did not
        come whole, and asked again the server may give it whole."""
        status, reason, headers = _answer._head(stream)
        unread = None
        try:
            body = _answer._body(stream, status, headers)
        except _answer._NotHTTP as wrong:
            unread = self._not_http(method, target, wrong)
        except _answer._TooLong:
            unread = self._too_long(method, target)
        if unread is not None:
            if 200 <= status < 300:
                raise unread
            body = b""
        wait = headers.get("retry-after", (None,))[0]
        return _Answer(status, reason, wait, body, forgotten, unread)

    def _once(self, method, target):
        """One request, on a connection of its own, and its answer."""
        import http.client

        connection = None
        try:
            # Within what a failure is told by: making ready for TLS reads
            # files, and fails as opening a connection does.
            connection = self._connection()
            connection.connect()
            if self._local_only:
                self._held_to_this_machine(connection)
            connection.sock.settimeout(_READ_SECONDS)
            # Counted as the request leaves: a refusal of a version that
            # comes after this is newer than what this request is answered.
            forgotten = self._forgotten
            connection.request(method, target, headers=self._headers)
            # The answer is read here, from the connection itself, and the
            # connection is let go of however reading it ends.
            with connection.sock.makefile("rb") as file:
                return self._read(_answer._Stream(file), method, target, forgotten)
        except (OSError, http.client.IncompleteRead) as failure:
            # What the system says of a connection names no more of the
            # server than its address, and never the token.
            cause = f"{type(failure).__name__}: {failure}".removesuffix(": ")
            raise ServerUnavailable(
                f"could not reach the server at {self.server.shown}: {cause}"
            ) from failure
        except http.client.HTTPException as failure:
            # What is no HTTP in `http.client`'s own terms, named by its
            # exception.
            raise self._not_http(method, target, type(failure).__name__) from failure
        except _answer._NotHTTP as wrong:
            # What is none in the client's, in its own words.
            raise self._not_http(method, target, wrong) from None
        finally:
            if connection is not None:
                connection.close()

    def _answered(self, method, target):
        """A request's answer, the request sent again while the server
        answers `503` with `Retry-After`, a few times."""
        attempt = 1
        while True:
            answer = self._once(method, target)
            if answer.status != 503 or attempt >= _ATTEMPTS:
                return answer
            wait = _retry_after(answer.retry_after)
            if wait is None:
                return answer
            time.sleep(wait)
            attempt += 1

    def _refusal(self, answer):
        """The exception an answer that is no success is raised as."""
        envelope = _envelope(answer.body)
        if envelope is None:
            raised = _RAISED_BY_STATUS.get(answer.status, RemoteError)
            if 300 <= answer.status < 400 and answer.status != 304:
                return raised(
                    f"the server at {self.server.shown} answered {answer.said}, a redirect, which "
                    "the client never follows; name the server by the URL it answers at",
                    status=answer.status,
                )
            return raised(
                f"the server at {self.server.shown} answered {answer.said} without QuantForge's "
                "error envelope; is it a QuantForge server?",
                status=answer.status,
            )
        code, message, supported = envelope
        raised = _RAISED.get(code, RemoteError)
        if raised is VersionMismatch:
            # The version agreed on is one the server no longer speaks: the
            # next request asks it again.
            with self._lock:
                self._forgotten += 1
                self.agreed = None
            return VersionMismatch(message, code=code, status=answer.status, supported=supported)
        return raised(message, code=code, status=answer.status)

    def _made_of(self, answer, method, target, read):
        """An answer as `read` makes of its JSON, or the exception it is
        raised as."""
        if not 200 <= answer.status < 300:
            raise self._refusal(answer) from answer.unread
        try:
            return read(_parsed(answer.body))
        except _Unreadable as unreadable:
            raise RemoteError(
                f"the server at {self.server.shown} answered {method} {target} with a body the "
                f"protocol does not read: {unreadable}; is it a QuantForge server?"
            ) from None

    def _ask(self, method, target, read):
        """A request's answer as `read` makes of its JSON, or the exception
        it failed with."""
        return self._made_of(self._answered(method, target), method, target, read)

    def handshake(self):
        """The server's release and the protocol version agreed with it, the
        newest both sides speak. A server that was asked is not asked
        again."""
        agreed = self.agreed
        if agreed is not None:
            return agreed

        def read(value):
            what = "the server's version"
            record = _object(value, what)
            return _member(record, "version", str, what), _versions(record, what)

        target = f"{self.server.path}version"
        answer = self._answered("GET", target)
        release, (oldest, newest) = self._made_of(answer, "GET", target, read)
        if newest < _MIN_SUPPORTED or oldest > _PROTO_VERSION:
            raise VersionMismatch(
                f"the server at {self.server.shown} runs QuantForge {release}, which speaks "
                f"protocol versions {oldest} to {newest}, and this package speaks "
                f"{_MIN_SUPPORTED} to {_PROTO_VERSION}; upgrade the older of the two",
                supported=(oldest, newest),
            )
        agreed = (release, min(newest, _PROTO_VERSION))
        # Kept, unless the server refused a version while this answer was on
        # its way: what it said before that is not its latest, and the
        # request it is used for here is the last it serves. What counts is
        # the request this answer came to, the last of them where the server
        # was busy and was asked again.
        with self._lock:
            if self._forgotten == answer.forgotten:
                self.agreed = agreed
        return agreed

    def ask(self, method, route, read):
        """The answer of a route under the agreed version, as `read` makes
        of it."""
        _, version = self.handshake()
        return self._ask(method, f"{self.server.path}v{version}/{route}", read)
