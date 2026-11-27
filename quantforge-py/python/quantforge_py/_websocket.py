"""A WebSocket as the client reads one, with the standard library alone: a
connection asked to become a stream, and the text messages a server sends
on it (`docs/python-jobs.md`, "How a log is followed").

A QuantForge server offers one stream, a job's log, and says everything on
it in text messages. The client says nothing on it but what WebSocket asks
of a client, so this is the half of WebSocket (RFC 6455) that takes:

- **The upgrade is held to its answer.** A server that answers anything but
  `101`, that does not name WebSocket, or whose answer is not to the key
  this client sent, has not opened a stream. Neither has one that names an
  extension or a protocol: the client asked for none.
- **A frame is as WebSocket writes one,** or the stream is broken: no
  reserved bit, no mask on what a server sends, a control frame short and
  whole, a message continued only once it has begun.
- **A message is taken to `_MOST_MESSAGE_BYTES`,** in however many frames it
  comes, and a text message is UTF-8.
- **A ping is answered, and a close is answered,** each as WebSocket asks,
  with frames that are masked, as a client's are.
- **A stream is let go of in order.** The client says goodbye, says that
  it sends no more, and reads what the server still sends to the server's
  own goodbye, for a moment and no longer: a connection closed over what
  it has not read is cut off instead of ended, and the server would not
  hear the goodbye.
- **A stream that falls silent for longer than the server may is broken.**
  A server pings a stream it has nothing to say on, so a silence that long
  is a connection that is gone.

A stream that cannot be had or cannot go on is a `_Broken`, or what the
system or `quantforge_py._answer` raised of the connection. None of them is
raised to a caller of the package: the log is read a page at a time from
where the stream stopped (`quantforge_py._logs`).

Importing this module imports nothing of the network.

This module is private.
"""

import time

from . import _answer

# What a server's answer to an upgrade is taken over, with the client's key
# before it: WebSocket's own constant.
_ACCEPT_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

# The most bytes of one message that are taken. A server keeps 8 KiB of a
# line, which JSON writes in six times as many bytes at the most.
_MOST_MESSAGE_BYTES = 1024 * 1024

# The longest a control frame's payload is: WebSocket's own limit.
_MOST_CONTROL_BYTES = 125

# The kinds of frame WebSocket has.
_CONTINUED, _TEXT, _BINARY, _CLOSE, _PING, _PONG = 0, 1, 2, 8, 9, 10

# What a close says when nothing went wrong.
_NORMAL = 1000

# How long a stream that is let go of is read for what its server still
# sends, in seconds.
_GOODBYE_SECONDS = 1.0


class _Broken(Exception):
    """A stream that could not be had, or cannot go on: why."""


def _listed(headers, name):
    """The words a header lists, in lower case."""
    return {
        word.strip(" \t").lower() for value in headers.get(name, ()) for word in value.split(",")
    }


def _opened(connect, target, identity, silence):
    """Asks `target` as a WebSocket, on the connection `connect` opens, and
    gives the stream once the server has answered that it is one.

    `identity` is the headers that say who asks, the token among them, and
    `silence` how long the stream may then stay silent, in seconds. The
    answer to the upgrade has as long as opening a connection has. Whatever
    stops the stream from opening is raised, and the connection closed.
    """
    import base64
    import hashlib
    import os

    key = base64.b64encode(os.urandom(16)).decode("ascii")
    asked = {
        **identity,
        "Connection": "Upgrade",
        "Upgrade": "websocket",
        "Sec-WebSocket-Key": key,
        "Sec-WebSocket-Version": "13",
    }
    connection = connect()
    try:
        connection.request("GET", target, headers=asked)
        file = connection.sock.makefile("rb")
        try:
            _, status, _, headers = _answer._head(_answer._Stream(file))
            if status != 101:
                raise _Broken(f"the server answered {status} to the upgrade")
            if "websocket" not in _listed(headers, "upgrade") or "upgrade" not in _listed(
                headers, "connection"
            ):
                raise _Broken("the answer to the upgrade does not name WebSocket")
            # Not a secret, and no hash of one: what WebSocket has a server
            # say to show that it read this client's upgrade.
            taken = hashlib.sha1((key + _ACCEPT_GUID).encode("ascii"), usedforsecurity=False)
            if headers.get("sec-websocket-accept") != [base64.b64encode(taken.digest()).decode()]:
                raise _Broken("the answer to the upgrade is not to this client's key")
            if "sec-websocket-extensions" in headers or "sec-websocket-protocol" in headers:
                raise _Broken("the answer to the upgrade names what the client did not ask for")
            connection.sock.settimeout(silence)
        except BaseException:
            file.close()
            raise
    except BaseException:
        connection.close()
        raise
    return _Socket(connection, file)


class _Socket:
    """An open stream: the text messages its server sends, each whole, and
    the goodbye that lets go of it."""

    __slots__ = ("_connection", "_file")

    def __init__(self, connection, file):
        self._connection = connection
        self._file = file

    def _take(self, count):
        """The next `count` bytes of the stream. Fewer came where the
        connection ended first."""
        taken = self._file.read(count)
        if len(taken) != count:
            raise _Broken(
                "the connection ended inside a frame" if taken else "the connection ended"
            )
        return taken

    def _send(self, kind, payload):
        """Sends one control frame, masked, as every frame of a client is."""
        import os

        mask = os.urandom(4)
        masked = bytes(byte ^ mask[at % 4] for at, byte in enumerate(payload))
        self._connection.sock.sendall(bytes((0x80 | kind, 0x80 | len(payload))) + mask + masked)

    def _frame(self):
        """The head of the next frame: whether it ends its message, its
        kind, and how many bytes it holds."""
        first, second = self._take(2)
        if first & 0x70:
            raise _Broken("a frame of an extension the client did not ask for")
        if second & 0x80:
            raise _Broken("a frame that is masked, which a server's never is")
        final, kind, length = bool(first & 0x80), first & 0x0F, second & 0x7F
        if kind >= _CLOSE:
            if not final or length > _MOST_CONTROL_BYTES:
                raise _Broken("a control frame that is long, or in pieces")
        elif length == 126:
            length = int.from_bytes(self._take(2), "big")
        elif length == 127:
            length = int.from_bytes(self._take(8), "big")
        return final, kind, length

    def message(self):
        """The next text message the server sent, or `None` once it has
        closed the stream. A ping is answered on the way, and a message
        that is no text is passed over: the protocol sends none."""
        pieces, held, text = None, 0, False
        while True:
            final, kind, length = self._frame()
            if kind in (_TEXT, _BINARY):
                if pieces is not None:
                    raise _Broken("a message that begins inside another")
                pieces, held, text = [], 0, kind == _TEXT
            elif kind == _CONTINUED:
                if pieces is None:
                    raise _Broken("a frame that continues no message")
            elif kind not in (_CLOSE, _PING, _PONG):
                raise _Broken(f"a frame of a kind WebSocket does not have, {kind}")
            # Told by what its frames say before any of one is read.
            if kind < _CLOSE:
                held += length
                if held > _MOST_MESSAGE_BYTES:
                    raise _Broken(f"a message of more than {_MOST_MESSAGE_BYTES} bytes")
            payload = self._take(length)
            if kind == _CLOSE:
                return None
            if kind == _PING:
                self._send(_PONG, payload)
            if kind >= _CLOSE:
                continue
            pieces.append(payload)
            if not final:
                continue
            whole, pieces = b"".join(pieces), None
            if text:
                try:
                    return whole.decode("utf-8")
                except UnicodeDecodeError:
                    raise _Broken("a text message that is not UTF-8") from None

    def close(self):
        """Says goodbye, as WebSocket has each side do, and lets go of the
        connection. A server that has sent its last frame waits for this to
        know that every frame arrived.

        What the server still sends is read and dropped until it has let
        go too, for `_GOODBYE_SECONDS` at the most, so that the connection
        ends where it would otherwise be cut off."""
        import socket

        sock = self._connection.sock
        try:
            self._send(_CLOSE, _NORMAL.to_bytes(2, "big"))
            sock.shutdown(socket.SHUT_WR)
            until = time.monotonic() + _GOODBYE_SECONDS
            while (left := until - time.monotonic()) > 0:
                sock.settimeout(left)
                if not self._file.read1(_answer._PIECE_BYTES):
                    break
        except OSError:
            # A connection that is gone already has nothing to be told.
            pass
        try:
            self._file.close()
        except OSError:
            pass
        self._connection.close()
