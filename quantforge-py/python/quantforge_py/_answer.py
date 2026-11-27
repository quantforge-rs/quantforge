"""One answer of a server as it is read from its connection, by the client
itself (`docs/python-client.md`, "How an answer is read").

`http.client` opens a connection and writes a request, and does not read the
answer. It reads a chunk of whatever size the chunk states, to the end of
the connection for a size below zero; it takes an answer in chunks that ends
before its last chunk does; it reads trailers without end; it knows
`chunked` in one spelling; and it stops at a `103`. What reads an answer
here holds it to what HTTP writes, and to limits of its own:

- **An answer says where its body ends, or it is not HTTP.** A body is as
  long as its `Content-Length` says, or comes in chunks, or ends where its
  connection does; an answer of a status that has no body has none. A
  length is digits and one number, a transfer coding is `chunked` or none,
  and a chunk states its size in hex digits and ends where that says. It
  is said once: no length stands beside chunks, and HTTP/1.0 has no
  transfer coding to name.
- **What frames a body is a line at a time,** each a status line, a header,
  a chunk's size or a trailer as HTTP writes one, and all of it counted: no
  more of it is taken than `_MOST_FRAMING_BYTES`.
- **A body is read in pieces of the client's own size,** to the longest
  that is taken, `_MOST_ANSWER_BYTES`, and one that is longer by its own
  account is not read at all. An answer that is a file is taken to the
  length its caller knows it by instead, which may be more or less.
- **A limit is what is taken.** To tell an answer that is past one, one
  byte past it is read, and no more.
- **An answer that only says another is to come is read past,** to the
  answer after it.
- **An answer that ends before it says it does was cut short.**

What is wrong with an answer is a `_NotHTTP` or a `_TooLong`, or one of the
exceptions `http.client` has for it, which `quantforge_py._wire` raises to
a caller as the client's own.

Importing this module imports nothing of the network.

This module is private.
"""

# The longest body of an answer that is taken, in bytes. A server's longest,
# a thousand jobs that each failed at the length a failure is cut to, is
# under this.
_MOST_ANSWER_BYTES = 64 * 1024 * 1024

# The most bytes of a body that one read asks for: a body is read in pieces
# of the client's own size, and held as it comes.
_PIECE_BYTES = 64 * 1024

# The most bytes that are taken of what frames an answer's body: its status
# line, its headers, the sizes of its chunks and its trailers, and any
# answer before it that only says one is to come. A server's own come to a
# few hundred bytes.
_MOST_FRAMING_BYTES = 1024 * 1024

# The longest line of those, its end included, and the most headers of one
# answer, or trailers: `http.client`'s own limits of the two.
_MOST_LINE_BYTES = 65536
_MOST_HEADERS = 100

# What a header's name is written in: a token, as HTTP calls it.
_NAME_BYTES = b"!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"

# What no line of what frames a body holds: a control character other than
# a tab, a carriage return that ends no line among them.
_CONTROL_BYTES = bytes(byte for byte in range(128) if (byte < 32 and byte != 9) or byte == 127)


class _NotHTTP(Exception):
    """An answer that is not written as HTTP writes one, or does not say
    where its body ends: what is wrong with it, as a refusal names it."""


class _TooLong(Exception):
    """An answer whose body is longer than is taken, by its own account or
    by what came."""


def _at_most(digits, most):
    """The number that ASCII digits write, or `most` where it is larger. A
    number longer than `most` is told by its length and never converted:
    Python refuses to read one of thousands of digits, and a server sends
    whatever it likes."""
    digits = digits.lstrip("0")
    if len(digits) > len(str(most)):
        return most
    return min(int(digits or "0"), most)


class _Stream:
    """One answer as it comes from its connection: what frames its body a
    line at a time, counted against the most that is taken of it, and its
    body in pieces of the client's own size."""

    __slots__ = ("_file", "_framing")

    def __init__(self, file):
        self._file = file
        # How much of what frames the body may still be taken.
        self._framing = _MOST_FRAMING_BYTES

    def line(self):
        """The next line of what frames the body, without its end, or `None`
        where the connection ended before it. A line ends with a line feed,
        with a carriage return before it or without, and holds no other
        control character than a tab."""
        most = min(_MOST_LINE_BYTES, self._framing)
        line = self._file.readline(most + 1)
        if not line:
            return None
        if len(line) > most:
            if most < _MOST_LINE_BYTES:
                raise _NotHTTP(f"more than {_MOST_FRAMING_BYTES} bytes beside its body")
            raise _NotHTTP(f"a line of more than {_MOST_LINE_BYTES} bytes")
        if not line.endswith(b"\n"):
            import http.client

            # The connection ended inside the line.
            raise http.client.IncompleteRead(line)
        self._framing -= len(line)
        line = line[:-2] if line.endswith(b"\r\n") else line[:-1]
        if line.translate(None, _CONTROL_BYTES) != line:
            raise _NotHTTP("a line with a control character in it")
        return line

    def into(self, body, count):
        """Adds the next `count` bytes of the body to `body`, a piece at a
        time. Fewer came where the connection ended first, and a read says
        nothing of that: a body cut short is an `IncompleteRead`."""
        left = count
        while left:
            piece = self._file.read1(min(_PIECE_BYTES, left))
            if not piece:
                import http.client

                raise http.client.IncompleteRead(bytes(body), left)
            body += piece
            left -= len(piece)

    def rest(self, most):
        """A body that ends where its connection does, which says nothing of
        its length: read to its end, and to one byte past `most`, the
        longest that is taken, at the most, which is how one that is longer
        is told."""
        body = bytearray()
        while len(body) <= most:
            piece = self._file.read1(min(_PIECE_BYTES, most + 1 - len(body)))
            if not piece:
                return bytes(body)
            body += piece
        raise _TooLong


def _status(stream):
    """An answer's status line, as the version it is written in, its status
    and its phrase."""
    import http.client

    line = stream.line()
    if line is None:
        # In `http.client`'s own words for it, which a caller may know.
        raise http.client.RemoteDisconnected("Remote end closed connection without response")
    words = line.split(None, 2)
    version = words[0] if words else b""
    if not version.startswith(b"HTTP/"):
        raise http.client.BadStatusLine(line.decode("iso-8859-1"))
    # HTTP/1.1, or 1.0, or a later revision of the two, which reads alike.
    if not (len(version) == 8 and version.startswith(b"HTTP/1.") and version[7:].isdigit()):
        raise http.client.UnknownProtocol(version.decode("iso-8859-1"))
    status = words[1] if len(words) > 1 else b""
    if not (len(status) == 3 and status.isdigit() and status[:1] != b"0"):
        raise http.client.BadStatusLine(line.decode("iso-8859-1"))
    reason = words[2].strip().decode("iso-8859-1") if len(words) > 2 else ""
    return version, int(status), reason


def _fields(stream):
    """The lines of an answer's headers, or of its trailers, to the empty
    line that ends them: each a name, a colon and a value, or one that goes
    on from the line before it. Headers that end before their empty line
    were cut short."""
    lines = []
    while True:
        line = stream.line()
        if line is None:
            import http.client

            raise http.client.IncompleteRead(b"")
        if not line:
            return lines
        if len(lines) >= _MOST_HEADERS:
            raise _NotHTTP(f"more than {_MOST_HEADERS} headers")
        name = line.partition(b":")[0]
        goes_on = bool(lines) and line[:1] in (b" ", b"\t")
        # A name is a token: anything else before the colon, a space among
        # it, and a line without a colon, are no header.
        if not goes_on and (name == line or not name or name.strip(_NAME_BYTES)):
            raise _NotHTTP("a header that is not a name and a value")
        lines.append(line)


def _headers(lines):
    """Headers as the values of each name, the name in lower case and a
    value without the space around it, which is no part of it. They are
    read here, from lines that were each held to being a header, and not
    handed to a parser that might read a line otherwise than it was held
    to."""
    headers = {}
    values = None
    for line in lines:
        if line[:1] in (b" ", b"\t"):
            # It goes on from the line before it.
            more = line.strip(b" \t").decode("iso-8859-1")
            values[-1] = f"{values[-1]} {more}".strip()
            continue
        name, _, value = line.partition(b":")
        values = headers.setdefault(name.decode("ascii").lower(), [])
        values.append(value.strip(b" \t").decode("iso-8859-1"))
    return headers


def _held_to_its_version(version, headers):
    """Refuses an answer of HTTP/1.0 that names a transfer coding. HTTP/1.0
    has none, and an answer written in it that names one is to be read as
    one whose framing is faulty, whatever else it is: one that only says
    another is to come, and one of a status that has no body, as much as
    one whose body would come in chunks."""
    if version == b"HTTP/1.0" and "transfer-encoding" in headers:
        raise _NotHTTP("a Transfer-Encoding in an answer of HTTP/1.0")


def _head(stream):
    """The version an answer is written in, its status, its phrase and its
    headers. An answer that only says another is to come, a `1xx` other
    than `101`, is read past, as HTTP asks of a client: `http.client` reads
    past a `100` alone. It is held to its version first, as the answer
    after it is where its body is read."""
    while True:
        version, status, reason = _status(stream)
        headers = _headers(_fields(stream))
        if status == 101 or not 100 <= status < 200:
            return version, status, reason, headers
        _held_to_its_version(version, headers)


def _has_no_body(status):
    """Whether an answer of this status has no body, whatever its headers
    say of one: a `204`, a `304` and a `101`. Such an answer may state the
    length of the body it stands for, and how that one would have come, in
    chunks among other ways; neither is its own."""
    return status in (101, 204, 304)


def _in_chunks(headers):
    """Whether an answer's body comes in chunks: `chunked` is its one
    transfer coding, in whatever case and with whatever space around it.
    `identity`, which HTTP once had for no coding, is none. Any other is one
    the client does not read, and with it nothing says where the body ends.

    An answer says once where its body ends: a length beside its chunks is
    what no sender may write, and is refused as well."""
    codings = [
        coding
        for value in headers.get("transfer-encoding", ())
        for coding in (word.strip(" \t").lower() for word in value.split(","))
        if coding and coding != "identity"
    ]
    if codings and codings != ["chunked"]:
        raise _NotHTTP("a Transfer-Encoding other than chunked")
    if codings and "content-length" in headers:
        raise _NotHTTP("a Content-Length beside a Transfer-Encoding")
    return bool(codings)


def _stated_length(headers, most):
    """How many bytes an answer says its body has, or `None` where it says
    none: at most one more than `most`, the longest body that is taken,
    which is how one that is too long is told. A length that is said more
    than once is one length where each says the same; one that is not
    digits, or two that differ, is a `_NotHTTP`, since nothing then says
    where the body ends."""
    values = headers.get("content-length")
    if values is None:
        return None
    stated = {part.strip(" \t") for value in values for part in value.split(",")}
    if not all(text.isascii() and text.isdigit() for text in stated):
        raise _NotHTTP("a Content-Length that is not digits")
    if len({text.lstrip("0") for text in stated}) != 1:
        raise _NotHTTP("two Content-Lengths that differ")
    return _at_most(stated.pop(), most + 1)


def _chunks(stream, most):
    """A body that comes in chunks, whole: each chunk of the size it states,
    in hex digits and nothing else, the last of no bytes, and the trailers
    after it to the empty line that ends them. A chunk that would make the
    body longer than `most`, the longest that is taken, is not read."""
    import http.client

    body = bytearray()
    while True:
        line = stream.line()
        if line is None:
            raise http.client.IncompleteRead(bytes(body))
        # What follows the size after a `;` is the chunk's own note, which
        # nothing here reads.
        size = line.partition(b";")[0].rstrip(b" \t")
        if not size or size.strip(b"0123456789abcdefABCDEF"):
            raise _NotHTTP("a chunk whose size is not hex digits")
        size = size.lstrip(b"0")
        if not size:
            break
        # Told by its digits before it is converted: more than eight is
        # more than any body that is taken, the longest file among them.
        if len(size) > 8 or len(body) + int(size, 16) > most:
            raise _TooLong
        stream.into(body, int(size, 16))
        end = stream.line()
        if end is None:
            raise http.client.IncompleteRead(bytes(body))
        if end:
            raise _NotHTTP("a chunk that does not end where its size says")
    _fields(stream)
    return bytes(body)


def _body(stream, version, status, headers, most=None):
    """An answer's body, whole. An answer of a status that has no body is
    read for none. One that comes in chunks is read by its chunks, and one
    that states its length to that length and no further, so that a server
    that keeps its connection open is not waited for; a body longer than is
    taken by either account is a `_TooLong`, and is not read. Any other
    ends where its connection does. Before any of that the answer is held
    to the version it is written in, whether it has a body or not.

    `most` is the longest body that is taken: `_MOST_ANSWER_BYTES`, unless
    the answer is a file whose length its caller knows."""
    if most is None:
        most = _MOST_ANSWER_BYTES
    _held_to_its_version(version, headers)
    if _has_no_body(status):
        return b""
    if _in_chunks(headers):
        return _chunks(stream, most)
    stated = _stated_length(headers, most)
    if stated is None:
        return stream.rest(most)
    if stated > most:
        raise _TooLong
    body = bytearray()
    stream.into(body, stated)
    return bytes(body)
