"""A job's log followed from Python, against servers of the test's own
(docs/python-jobs.md): what `Job.stream_logs` takes and gives, the stream
it asks for and what it says on it, every way a stream stops and the pages
that take over from where it stopped, and the cursor that keeps a line from
being given twice or left out.

A stand-in here answers the log's route as a WebSocket with the frames a
test tells it to send, or with whatever bytes a test would have a server
send in their place, and as pages where it is asked plainly. What the
client does with the stream of a real `serve run` is in
`test_handle_jobs.py`.
"""

import copy
import pickle
import struct
import threading
import time
import types

import pytest

import quantforge_py as qf
from quantforge_py import _logs, _websocket, _wire
from standin import (
    BINARY,
    CLOSE,
    CONTINUED,
    DROP,
    JOB,
    PING,
    PONG,
    TEXT,
    StandIn,
    closing,
    frame,
    refusal,
    upgraded,
    written,
)

LOGS = f"/v1/backtests/{JOB}/logs"
RUNNING = {"kind": "running", "started_at_ms": 1_792_454_401_500}

# A token no line could hold by chance, and no scanner takes for a secret.
TOKEN = "a-sentinel-token-no-line-holds"

# The goodbye a client sends: WebSocket's close, with the code that says
# nothing went wrong.
GOODBYE = (CLOSE, struct.pack(">H", 1000))


def line(seq, message=None, level="info", **more) -> dict:
    """A line of a log as the protocol writes it."""
    return {
        "seq": seq,
        "at_ms": 1_792_454_400_000 + seq,
        "level": level,
        "message": f"line {seq}" if message is None else message,
        **more,
    }


def said(kind, **members) -> bytes:
    """A frame of the log's stream, as a server sends it."""
    return frame({"type": kind, **members})


def a_line(seq, **more) -> bytes:
    return said("line", **line(seq, **more))


def page(*seqs, dropped=0, after=None, progress=None, complete=False, **changed) -> dict:
    """A page of the log as the protocol writes it: the lines of these
    numbers, and the cursor after the last of them unless it is given."""
    if after is None:
        after = max(seqs, default=0)
    written = {
        "job_id": JOB,
        "lines": [line(seq) for seq in seqs],
        "dropped_lines": dropped,
        "next_after": after,
        "complete": complete,
    }
    if progress is not None:
        written["progress"] = dict(zip(("bars_done", "bars_total"), progress))
    written.update(changed)
    return written


@pytest.fixture
def server():
    with StandIn() as stand_in:
        stand_in.says("GET", f"/v1/backtests/{JOB}", (200, written(state=RUNNING)))
        yield stand_in


@pytest.fixture
def job(server):
    return qf.Client(server.url).backtest.get(JOB)


@pytest.fixture
def no_waiting(monkeypatch):
    """Records how long a follower would have waited before its next page,
    in the place of waiting."""
    waited = []
    monkeypatch.setattr(_logs, "time", types.SimpleNamespace(sleep=waited.append))
    return waited


def routes(server) -> list[str]:
    """What a server was asked once the client had its job."""
    assert server.routes[:2] == ["GET /version", f"GET /v1/backtests/{JOB}"]
    return [route.removeprefix("GET /v1/backtests/" + JOB) for route in server.routes[2:]]


def numbers(lines) -> list[int]:
    return [each.seq for each in lines]


def heard(server, streams=1):
    """What the client sent on each stream it was given, once it let go of
    them all."""
    deadline = time.monotonic() + 10
    while len(server.heard) < streams:
        assert time.monotonic() < deadline, "the client let go of its stream"
        time.sleep(0.005)
    return server.heard


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


# What a follower takes, and when it asks.


def test_a_log_is_followed_by_an_iterator_that_asks_nothing_until_it_is_asked_for_a_line(
    server, job
):
    server.streams(f"{LOGS}?after=0", a_line(1), said("end"), closing())
    logs = job.stream_logs()
    assert iter(logs) is logs and isinstance(logs, types.GeneratorType)
    assert routes(server) == []
    first = next(logs)
    assert routes(server) == ["/logs?after=0 (stream)"]
    assert type(first) is qf.LogLine and first.seq == 1
    assert list(logs) == []
    # The job's status is what it was: following a log asks for no status.
    assert job.status.state == "running" and routes(server) == ["/logs?after=0 (stream)"]


def test_what_a_follower_takes_is_checked_as_it_is_called_and_before_anything_is_asked(
    server, job, refused
):
    for after, kind, message in [
        ("3", TypeError, "after must be an int, not str"),
        (3.0, TypeError, "after must be an int, not float"),
        (None, TypeError, "after must be an int, not NoneType"),
        (True, TypeError, "after must be an int, not bool"),
        (
            -1,
            ValueError,
            "after is negative: it is the number of the last line the caller holds, or 0 for the "
            "log from its first line",
        ),
        (-(10**5000), ValueError, None),
        (2**63, ValueError, "after is beyond the number any line of a log has"),
        (10**5000, ValueError, "after is beyond the number any line of a log has"),
    ]:
        if message is None:
            with pytest.raises(kind, match="after is negative"):
                job.stream_logs(after=after)
        else:
            refused(kind, message, job.stream_logs, after=after)
    for given in (3, "print", [print]):
        refused(
            TypeError,
            f"on_progress must be a callable or None, not {type(given).__name__}",
            job.stream_logs,
            on_progress=given,
        )
    # Both are named, and neither is taken by its place.
    with pytest.raises(TypeError):
        job.stream_logs(3)
    assert routes(server) == []


def test_a_log_is_followed_from_the_line_after_the_one_its_caller_holds(server, job):
    server.streams(f"{LOGS}?after=41", a_line(42), a_line(43), said("end"), closing())
    assert numbers(job.stream_logs(after=41)) == [42, 43]
    assert routes(server) == ["/logs?after=41 (stream)"]
    # The largest number a line has is one a log can be followed from.
    last = 2**63 - 1
    server.streams(f"{LOGS}?after={last}", said("end"), closing())
    assert list(job.stream_logs(after=last)) == []


# The stream.


def test_a_stream_is_asked_for_as_websocket_asks_and_says_who_asks(server):
    server.streams(f"{LOGS}?after=0", said("end"), closing())
    client = qf.Client(server.url, token=TOKEN)
    assert list(client.backtest.get(JOB).stream_logs()) == []
    method, target, headers, sent = server.asked[-1]
    assert (method, target, sent) == ("GET", f"{LOGS}?after=0", b"")
    assert headers["Upgrade"] == "websocket" and headers["Connection"] == "Upgrade"
    assert headers["Sec-WebSocket-Version"] == "13"
    # A key of sixteen bytes, as WebSocket has it, and a new one each time.
    import base64

    assert len(base64.b64decode(headers["Sec-WebSocket-Key"], validate=True)) == 16
    assert headers["User-Agent"] == f"quantforge-py/{qf.__version__}"
    # The token goes with the stream as with every request, and the client
    # asks for no extension and no protocol of its own.
    assert headers["Authorization"] == f"Bearer {TOKEN}"
    assert not {"Sec-WebSocket-Extensions", "Sec-WebSocket-Protocol", "Accept"} & set(headers)
    assert list(client.backtest.get(JOB).stream_logs()) == []
    assert server.asked[-1][2]["Sec-WebSocket-Key"] != headers["Sec-WebSocket-Key"]
    # Without a token it sends none.
    assert list(qf.Client(server.url).backtest.get(JOB).stream_logs()) == []
    assert "Authorization" not in server.asked[-1][2]


def test_a_stream_gives_each_line_as_the_server_wrote_it_and_ends_with_the_log(server, job):
    server.streams(
        f"{LOGS}?after=0",
        a_line(1, message="started"),
        a_line(2, message='a line\twith "quotes" and ünïcode\x1b[0m', level="warning"),
        a_line(3, message="the start of a very long line", level="error", dropped_bytes=1040384),
        a_line(4, message="", level="debug", dropped_bytes=None),
        a_line(5, level="trace"),
        said("end"),
        closing(),
    )
    lines = list(job.stream_logs())
    assert [(each.seq, each.level, each.message, each.dropped_bytes) for each in lines] == [
        (1, "info", "started", None),
        (2, "warning", 'a line\twith "quotes" and ünïcode\x1b[0m', None),
        (3, "error", "the start of a very long line", 1040384),
        (4, "debug", "", None),
        # A level a later revision of the protocol adds is given as written.
        (5, "trace", "line 5", None),
    ]
    assert [each.at_ms for each in lines] == [1_792_454_400_000 + seq for seq in range(1, 6)]
    # The log ended on the stream: no page was asked for, and the client
    # said goodbye, as WebSocket has each side do, in a frame that is masked.
    assert routes(server) == ["/logs?after=0 (stream)"]
    assert heard(server) == [[GOODBYE]]


def test_a_stream_is_read_past_what_is_not_a_line_and_in_however_many_frames_a_message_comes(
    server, job
):
    whole = a_line(2)
    text = whole[2:]
    long = "x" * 70_000
    server.streams(
        f"{LOGS}?after=0",
        a_line(1),
        # A ping is answered with what it holds.
        frame(b"are you there", PING),
        # A message in three frames, with a ping between two of them.
        frame(text[:10], TEXT, final=False),
        frame(text[10:20], CONTINUED, final=False),
        frame(b"", PING),
        frame(text[20:], CONTINUED),
        # A pong nobody asked for, a message that is no text, a frame type
        # a later revision of the protocol adds, and a member one adds.
        frame(b"", PONG),
        frame(b"\x00\x01", BINARY),
        frame(b"\x00", BINARY, final=False),
        frame(b"\x01", CONTINUED),
        said("checkpoint", bars_done=5, note="from a later revision"),
        said("line", **line(3), source="stdout"),
        # A message longer than a short frame holds, and than a medium one.
        a_line(4, message="y" * 300),
        a_line(5, message=long),
        said("end", state="succeeded"),
        closing(),
    )
    lines = list(job.stream_logs())
    assert numbers(lines) == [1, 2, 3, 4, 5]
    assert (lines[3].message, lines[4].message) == ("y" * 300, long)
    assert routes(server) == ["/logs?after=0 (stream)"]
    assert heard(server) == [[(PONG, b"are you there"), (PONG, b""), GOODBYE]]


def test_an_upgrade_is_taken_however_its_answer_spells_websocket(server, job):
    spelled = [
        dict(Upgrade="WebSocket", Connection="upgrade"),
        dict(Upgrade="websocket", Connection="keep-alive, Upgrade"),
        dict(Upgrade="h2c, WEBSOCKET", Connection="UPGRADE"),
    ]
    for headers in spelled:
        server.streams(
            f"{LOGS}?after=0",
            a_line(1),
            said("end"),
            closing(),
            answer=lambda nonce, headers=headers: upgraded(nonce, **headers),
        )
        assert numbers(job.stream_logs()) == [1]
    # An answer that only says another is to come is read past, to the one
    # that takes the upgrade.
    early = b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>\r\n\r\n"
    server.streams(
        f"{LOGS}?after=0",
        a_line(1),
        said("end"),
        closing(),
        answer=lambda nonce: early + upgraded(nonce),
    )
    assert numbers(job.stream_logs()) == [1]
    assert routes(server) == ["/logs?after=0 (stream)"] * 4


def test_progress_goes_to_the_callback_when_the_job_has_come_further_and_never_to_the_lines(
    server, job
):
    server.streams(
        f"{LOGS}?after=0",
        said("progress", bars_done=0, bars_total=1440),
        a_line(1),
        said("progress", bars_done=720, bars_total=1440),
        said("progress", bars_done=720, bars_total=1440),
        a_line(2),
        said("progress", bars_done=1440, bars_total=1440),
        said("end"),
        closing(),
    )
    told = []
    order = []

    def on_progress(bars_done, bars_total):
        told.append((bars_done, bars_total))
        order.append(("progress", bars_done))

    for each in job.stream_logs(on_progress=on_progress):
        order.append(("line", each.seq))
    assert told == [(0, 1440), (720, 1440), (1440, 1440)]
    assert all(type(value) is int for pair in told for value in pair)
    # Each is told where the server said it, between the lines around it.
    assert order == [
        ("progress", 0),
        ("line", 1),
        ("progress", 720),
        ("line", 2),
        ("progress", 1440),
    ]
    # Without a callback the frames are read, and nothing is told.
    assert numbers(job.stream_logs()) == [1, 2]
    # A method, a partial and a callable object are each a callback.
    for callback in (told.append.__self__.__class__().append, print):
        assert callable(callback)


def test_a_callback_that_raises_ends_the_log_with_what_it_raised_and_lets_go_of_the_stream(
    server, job
):
    server.streams(
        f"{LOGS}?after=0",
        a_line(1),
        said("progress", bars_done=1, bars_total=2),
        a_line(2),
        said("end"),
        closing(),
    )

    def fails(bars_done, bars_total):
        raise OSError("the caller's own")

    logs = job.stream_logs(on_progress=fails)
    assert next(logs).seq == 1
    # What a callback raises is the caller's, whatever it is: it is not
    # taken for a stream that broke, and no page is asked for.
    assert str(raised(OSError, lambda: next(logs))) == "the caller's own"
    assert list(logs) == []
    assert routes(server) == ["/logs?after=0 (stream)"]
    assert heard(server) == [[GOODBYE]]


def test_leaving_the_loop_lets_go_of_the_stream(server, job):
    held = threading.Event()

    def holds(nonce):
        # More than this the server never sends: the client leaves.
        held.wait(10)

    server.streams(f"{LOGS}?after=0", a_line(1), a_line(2), holds)
    logs = job.stream_logs()
    assert next(logs).seq == 1
    # The server says nothing more and does not let go: the client waits
    # for it a moment, and no longer.
    began = time.monotonic()
    logs.close()
    assert time.monotonic() - began < 5
    held.set()
    assert heard(server) == [[GOODBYE]]
    assert list(logs) == [] and routes(server) == ["/logs?after=0 (stream)"]
    # A loop that is left by `break` lets go of it as the iterator goes.
    held.clear()
    for each in job.stream_logs():
        break
    held.set()
    assert heard(server, 2)[1] == [GOODBYE]


class Let:
    """A connection as a stream lets go of it, in the place of a real one:
    what it has still to be read, a piece at a time, and everything that
    was done to it, in order. `raises` is what sending on it fails with,
    and `clock` what its reads take, by a clock of the test's own."""

    def __init__(self, *pieces, raises=None, clock=None, each=0.0):
        self.done = []
        self._pieces = list(pieces)
        self._raises = raises
        self._clock = clock
        self._each = each
        self.sock = self

    def sendall(self, sent):
        if self._raises is not None:
            raise self._raises
        self.done.append(("sent", bytes(sent)))

    def shutdown(self, how):
        self.done.append(("shutdown", how))

    def settimeout(self, seconds):
        self.done.append(("timeout", seconds))

    def read1(self, most):
        assert most > 0
        if self._clock is not None:
            self._clock.now += self._each
        piece = self._pieces.pop(0) if self._pieces else b""
        if isinstance(piece, BaseException):
            raise piece
        self.done.append(("read", len(piece)))
        return piece

    def close(self):
        self.done.append(("closed",))


def goodbye_of(sent: bytes) -> tuple[int, bytes]:
    """A frame a client sent, as its kind and what it holds, unmasked."""
    assert sent[0] & 0x80 and sent[1] & 0x80 and len(sent) == 6 + (sent[1] & 0x7F)
    mask, held = sent[2:6], sent[6:]
    return sent[0] & 0x0F, bytes(byte ^ mask[at % 4] for at, byte in enumerate(held))


def test_a_stream_is_let_go_of_in_order_so_that_its_connection_ends_and_is_not_cut_off():
    # It says goodbye, says that it sends no more, reads what the server
    # still sends to the server's own end, and only then closes: a
    # connection closed over what it has not read is cut off, and the
    # server does not hear the goodbye.
    import socket

    let = Let(b"x" * 10, b"y")
    _websocket._Socket(let, let).close()
    (_, sent), *rest = let.done
    assert goodbye_of(sent) == GOODBYE
    kinds = [step[0] for step in rest]
    assert (
        kinds
        == ["shutdown", "timeout", "read", "timeout", "read", "timeout", "read"] + ["closed"] * 2
    )
    assert rest[0] == ("shutdown", socket.SHUT_WR)
    assert [step[1] for step in rest if step[0] == "read"] == [10, 1, 0]
    # Each read has what is left of the moment, and no more.
    waits = [step[1] for step in rest if step[0] == "timeout"]
    assert all(0 < wait <= _websocket._GOODBYE_SECONDS for wait in waits)
    assert waits == sorted(waits, reverse=True)


def test_a_server_that_goes_on_sending_is_read_for_a_moment_and_no_longer(monkeypatch):
    clock = types.SimpleNamespace(now=100.0)
    monkeypatch.setattr(_websocket, "time", types.SimpleNamespace(monotonic=lambda: clock.now))
    # Each read takes three tenths of a second, by the test's clock, and
    # there is always more: four of them fill the second.
    let = Let(*[b"more"] * 50, clock=clock, each=0.3)
    _websocket._Socket(let, let).close()
    reads = [step for step in let.done if step[0] == "read"]
    assert len(reads) == 4 and _websocket._GOODBYE_SECONDS == 1.0
    assert [step[1] for step in let.done if step[0] == "timeout"] == pytest.approx(
        [1.0, 0.7, 0.4, 0.1]
    )
    assert let.done[-2:] == [("closed",), ("closed",)]


def test_a_connection_that_is_gone_is_let_go_of_without_a_word():
    for gone in (BrokenPipeError(), ConnectionResetError(), TimeoutError("timed out")):
        let = Let(b"never read", raises=gone)
        _websocket._Socket(let, let).close()
        assert let.done == [("closed",), ("closed",)]
    # One that goes as it is read, or stays silent, is let go of there.
    for gone in (ConnectionResetError(), TimeoutError("timed out")):
        let = Let(b"x", gone, b"never read")
        _websocket._Socket(let, let).close()
        assert [step[0] for step in let.done] == [
            "sent",
            "shutdown",
            "timeout",
            "read",
            "timeout",
            "closed",
            "closed",
        ]


# The cursor.


def test_no_line_is_given_twice_and_lines_the_server_no_longer_holds_move_the_cursor(
    server, job, no_waiting
):
    server.streams(
        f"{LOGS}?after=0",
        said("truncated", dropped_lines=9000),
        a_line(9001),
        a_line(9002),
        # A line at or before the cursor is one the caller has.
        a_line(9002),
        a_line(17),
        said("truncated", dropped_lines=5),
        # The stream stops here, without `end`: the pages take over from
        # the cursor, which stands past the lines that were dropped.
        closing(1001, "the server is stopping; ask again once it is back"),
    )
    server.says(
        "GET",
        f"{LOGS}?after=9007",
        # A page that counts more dropped lines, and repeats a line.
        (200, page(9010, 9011, dropped=2, after=9011)),
    )
    server.says("GET", f"{LOGS}?after=9011", (200, page(9011, 9012, complete=True)))
    assert numbers(job.stream_logs()) == [9001, 9002, 9010, 9011, 9012]
    assert routes(server) == [
        "/logs?after=0 (stream)",
        "/logs?after=9007",
        "/logs?after=9011",
    ]
    assert no_waiting == []


def test_progress_is_told_once_across_the_stream_and_the_pages_that_take_over(
    server, job, no_waiting
):
    server.streams(
        f"{LOGS}?after=0",
        a_line(1),
        said("progress", bars_done=5, bars_total=10),
        closing(),
    )
    server.says("GET", f"{LOGS}?after=1", (200, page(2, progress=(5, 10))))
    server.says("GET", f"{LOGS}?after=2", (200, page(3, progress=(10, 10), complete=True)))
    told = []
    lines = job.stream_logs(on_progress=lambda *progress: told.append(progress))
    assert numbers(lines) == [1, 2, 3]
    assert told == [(5, 10), (10, 10)]


# Every way a stream stops: the pages take over from where it stopped.

BROKEN_MID_STREAM = {
    "closed as the server stops": [closing(1001, "the server is stopping")],
    "closed without end": [closing()],
    "closed without a code": [frame(b"", CLOSE)],
    "an error frame": [said("error", code="internal", message="no"), closing(1011)],
    "an error frame alone": [said("error", code="unavailable", message="later")],
    "the connection gone": [DROP],
    "the connection gone inside a frame": [a_line(2)[:-3], DROP],
    "the connection gone inside a frame's head": [b"\x81", DROP],
    "a masked frame": [frame({"type": "line", **line(2)}, mask=b"mask")],
    "a masked frame whose mask changes nothing": [
        frame({"type": "line", **line(2)}, mask=b"\0\0\0\0")
    ],
    "a reserved bit": [frame({"type": "line", **line(2)}, reserved=0x40)],
    "not json": [frame("not json")],
    "json that is no object": [frame("[]")],
    "a constant json has not": [frame('{"type": "line", "seq": NaN}')],
    "no type": [frame({})],
    "a type that is no text": [frame({"type": 3})],
    "a line numbered zero": [said("line", **line(0))],
    "a line with a number that is text": [said("line", **{**line(2), "seq": "2"})],
    "a line with a number that is a flag": [said("line", **{**line(2), "seq": True})],
    "a line without its text": [said("line", seq=2, at_ms=1, level="info")],
    "a line without its time": [said("line", seq=2, level="info", message="m")],
    "a line without its level": [said("line", seq=2, at_ms=1, message="m")],
    "a line whose level is no text": [said("line", **{**line(2), "level": 3})],
    "a line cut by a negative count": [said("line", **line(2, dropped_bytes=-1))],
    "progress that is negative": [said("progress", bars_done=-1, bars_total=10)],
    "progress without its total": [said("progress", bars_done=1)],
    "dropped lines without their count": [said("truncated")],
    "dropped lines counted as text": [said("truncated", dropped_lines="3")],
    "text that is not utf-8": [frame(b"\xff\xfe{}")],
    "a line whose text is not utf-8": [
        frame(b'{"type":"line","seq":2,"at_ms":1,"level":"info","message":"caf\xe9"}')
    ],
    "a frame that continues nothing": [frame({"type": "line", **line(2)}, CONTINUED)],
    "a message begun inside another": [frame(b'{"ty', TEXT, final=False), a_line(2)],
    "a control frame in pieces": [frame(b"", PING, final=False)],
    "a control frame that is long": [frame(b"x" * 126, PING)],
    "a close that is long": [frame(b"x" * 126, CLOSE)],
    "a kind websocket has not": [frame(b"", 3)],
    "another kind websocket has not": [frame(b"", 11)],
}


@pytest.mark.parametrize("stopped", BROKEN_MID_STREAM, ids=str)
def test_a_stream_that_stops_without_end_is_followed_from_its_last_line_a_page_at_a_time(
    server, job, no_waiting, stopped
):
    sent = BROKEN_MID_STREAM[stopped]
    # Unless what stopped it let go of the connection, the stream goes on
    # to say that the log has ended, which a follower that stopped never
    # reads: one that read on past what stopped it would end there, with
    # the first line alone and no page asked for.
    last = sent[-1] if sent else None
    let_go = last is DROP or (last is not None and last[0] & 0x0F == CLOSE)
    after = [] if let_go else [said("end"), closing()]
    server.streams(f"{LOGS}?after=0", a_line(1), *sent, *after)
    server.says("GET", f"{LOGS}?after=1", (200, page(2, 3)))
    server.says("GET", f"{LOGS}?after=3", (200, page(complete=True, after=3)))
    assert numbers(job.stream_logs()) == [1, 2, 3]
    # Nothing a stream stops with is raised, and no line is asked for twice.
    assert routes(server) == ["/logs?after=0 (stream)", "/logs?after=1", "/logs?after=3"]
    assert no_waiting == []


def test_a_message_longer_than_is_taken_stops_the_stream_before_it_is_read(
    server, job, monkeypatch
):
    monkeypatch.setattr(_websocket, "_MOST_MESSAGE_BYTES", 256)
    long = a_line(2, message="x" * 300)
    pieces = [frame(long[4:104], TEXT, final=False), frame(long[104:], CONTINUED)]
    for sent in (
        [long],
        # In two frames that are each within it.
        pieces,
        # By what its frame says alone: the frame's head, and nothing of
        # what it says is to come.
        [struct.pack(">BBQ", 0x81, 127, 2**62)],
    ):
        server.streams(f"{LOGS}?after=0", a_line(1), *sent)
    server.says("GET", f"{LOGS}?after=1", (200, page(2, complete=True)))
    for _ in range(3):
        assert numbers(job.stream_logs()) == [1, 2]
    # One that is exactly as long as is taken is taken.
    exact = a_line(2, message="x" * 300)
    monkeypatch.setattr(_websocket, "_MOST_MESSAGE_BYTES", len(exact) - 4)
    server.streams(f"{LOGS}?after=1", exact, said("end"), closing())
    assert numbers(job.stream_logs(after=1)) == [2]


def test_a_stream_that_falls_silent_for_longer_than_a_server_may_is_given_up(
    server, job, monkeypatch
):
    # A second, and a server that says something every fifth of it below:
    # far enough apart that a busy machine's late timers part the two.
    monkeypatch.setattr(_wire, "_READ_SECONDS", 1.0)
    done = threading.Event()

    def silent(nonce):
        done.wait(10)

    server.streams(f"{LOGS}?after=0", a_line(1), silent)
    server.says("GET", f"{LOGS}?after=1", (200, page(2, complete=True)))
    began = time.monotonic()
    assert numbers(job.stream_logs()) == [1, 2]
    took = time.monotonic() - began
    done.set()
    # By a clock that may tick seldom, as Windows' does.
    assert 0.9 <= took < 8, took

    # A stream that says something within that time, a ping among it, goes
    # on for as long as it does.
    def slowly(nonce):
        time.sleep(0.2)
        return frame(b"", PING)

    server.streams(f"{LOGS}?after=0", slowly, slowly, slowly, a_line(1), said("end"), closing())
    began = time.monotonic()
    assert numbers(job.stream_logs()) == [1]
    assert time.monotonic() - began >= 0.5
    assert routes(server)[-1] == "/logs?after=0 (stream)"


REFUSED_UPGRADES = {
    "another key's answer": lambda nonce: upgraded(
        nonce, Sec_WebSocket_Accept="AAAAAAAAAAAAAAAAAAAAAAAAAAA="
    ),
    "no answer to the key": lambda nonce: upgraded(nonce, Sec_WebSocket_Accept=None),
    "no upgrade named": lambda nonce: upgraded(nonce, Upgrade=None),
    "another upgrade named": lambda nonce: upgraded(nonce, Upgrade="h2c"),
    "a connection that stays http": lambda nonce: upgraded(nonce, Connection="keep-alive"),
    "no connection named": lambda nonce: upgraded(nonce, Connection=None),
    "an extension nobody asked for": lambda nonce: upgraded(
        nonce, Sec_WebSocket_Extensions="permessage-deflate"
    ),
    "a protocol nobody asked for": lambda nonce: upgraded(nonce, Sec_WebSocket_Protocol="chat"),
    "a success in the place of an upgrade": lambda nonce: (
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"
    ),
    "a success that says it is an upgrade": lambda nonce: upgraded(nonce).replace(
        b"101 Switching Protocols", b"200 OK"
    ),
    "a refusal that says it is an upgrade": lambda nonce: upgraded(nonce).replace(
        b"101 Switching Protocols", b"404 Not Found"
    ),
    "a refusal a proxy made": lambda nonce: (
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n"
    ),
    "a proxy that has no upgrade": lambda nonce: b"HTTP/1.1 426 Upgrade Required\r\n\r\n",
    "a gateway that reached nothing": lambda nonce: b"HTTP/1.1 502 Bad Gateway\r\n\r\n",
    "what is not http": lambda nonce: b"SSH-2.0-OpenSSH_9.9\r\n",
    "an answer cut short": lambda nonce: upgraded(nonce)[:40],
    "no answer": lambda nonce: DROP,
    "a header that is none": lambda nonce: b"HTTP/1.1 101 Switching Protocols\r\nnonsense\r\n\r\n",
}


@pytest.mark.parametrize("answered", REFUSED_UPGRADES, ids=str)
def test_a_stream_that_cannot_be_had_is_read_a_page_at_a_time_from_the_start(
    server, job, no_waiting, answered
):
    sent = [DROP] if answered == "an answer cut short" else [a_line(5), said("end")]
    server.streams(f"{LOGS}?after=4", *sent, answer=REFUSED_UPGRADES[answered])
    server.says("GET", f"{LOGS}?after=4", (200, page(5, 6, complete=True)))
    assert numbers(job.stream_logs(after=4)) == [5, 6]
    assert routes(server) == ["/logs?after=4 (stream)", "/logs?after=4"]
    # Nothing was said on a stream that never opened: the client let go of
    # the connection, over whatever the server had sent on it.
    assert heard(server) in ([[]], [None], [DROP])


def test_an_upgrade_has_as_long_to_be_answered_as_a_connection_has_to_open(
    server, job, monkeypatch
):
    monkeypatch.setattr(_wire, "_CONNECT_SECONDS", 0.3)
    done = threading.Event()

    def never(nonce):
        done.wait(10)
        return b""

    server.streams(f"{LOGS}?after=0", answer=never)
    server.says("GET", f"{LOGS}?after=0", (200, page(1, complete=True)))
    began = time.monotonic()
    assert numbers(job.stream_logs()) == [1]
    took = time.monotonic() - began
    done.set()
    assert 0.25 <= took < 5, took


def test_a_server_that_offers_no_stream_is_read_a_page_at_a_time(server, job, no_waiting):
    # A server, or what stands in front of one, that answers the route as
    # it answers one asked plainly: with a page.
    server.says(
        "GET",
        f"{LOGS}?after=0",
        (200, page(1, 2)),
        (200, page(1, 2)),
    )
    server.says("GET", f"{LOGS}?after=2", (200, page(3, complete=True)))
    assert numbers(job.stream_logs()) == [1, 2, 3]
    assert routes(server) == ["/logs?after=0 (stream)", "/logs?after=0", "/logs?after=2"]


# The pages.


def test_pages_are_asked_for_one_after_another_and_waited_for_only_where_nothing_is_new(
    server, job, no_waiting
):
    server.says(
        "GET",
        f"{LOGS}?after=0",
        # Asked as a stream, which this server does not offer; then a page
        # of a job that has not started, twice; then its first lines.
        (200, page()),
        (200, page()),
        (200, page(progress=(0, 10))),
        (200, page(1, 2, progress=(3, 10))),
    )
    server.says(
        "GET",
        f"{LOGS}?after=2",
        # A quiet job, then a page whose cursor moved though it holds no
        # line, then the end.
        (200, page(after=2)),
        (200, page(after=7, dropped=5)),
    )
    server.says("GET", f"{LOGS}?after=7", (200, page(8, complete=True, progress=(10, 10))))
    told = []
    lines = job.stream_logs(on_progress=lambda *progress: told.append(progress))
    assert numbers(lines) == [1, 2, 8]
    assert told == [(0, 10), (3, 10), (10, 10)]
    # It waited after each page that moved the cursor no further, and after
    # no other.
    assert no_waiting == [_logs._PAGE_SECONDS] * 3 == [0.5] * 3
    assert routes(server) == [
        "/logs?after=0 (stream)",
        "/logs?after=0",
        "/logs?after=0",
        "/logs?after=0",
        "/logs?after=2",
        "/logs?after=2",
        "/logs?after=7",
    ]


def test_a_page_that_repeats_what_the_caller_holds_gives_nothing_and_is_waited_after(
    server, job, no_waiting
):
    server.says("GET", f"{LOGS}?after=0", (200, page()), (200, page(1, 2)))
    server.says(
        "GET",
        f"{LOGS}?after=2",
        # A server that gives the same lines again, with a cursor that went
        # back: nothing of it is given, and it is not asked without end.
        (200, page(1, 2, after=1)),
        (200, page(2, 3, after=3)),
    )
    server.says("GET", f"{LOGS}?after=3", (200, page(complete=True, after=3)))
    assert numbers(job.stream_logs()) == [1, 2, 3]
    assert no_waiting == [0.5]


def test_the_log_of_a_job_that_wrote_nothing_is_no_line(server, job, no_waiting):
    server.streams(f"{LOGS}?after=0", said("end"), closing())
    assert list(job.stream_logs()) == []
    # Nor, a page at a time.
    server.says("GET", f"{LOGS}?after=5", (200, page()), (200, page(complete=True, after=5)))
    assert list(job.stream_logs(after=5)) == []


UNREADABLE_PAGES = [
    ([], "a page of a job's log is not an object"),
    ({**page(1), "job_id": "no such job"}, "the `job_id` of a page of a job's log is no job ID"),
    ({**page(1), "job_id": 7}, "the `job_id` of a page of a job's log is not a string"),
    (
        {**page(1), "job_id": "0" * 32},
        f"it is a page of job {'0' * 32}'s log, not of job {JOB}'s",
    ),
    ({**page(1), "lines": {}}, f"the `lines` of job {JOB}'s log is not a list"),
    ({**page(1), "lines": [7]}, f"a line of job {JOB}'s log is not an object"),
    (
        {**page(1), "lines": [line(0)]},
        f"the `seq` of a line of job {JOB}'s log is no line's number, which is 1 or more",
    ),
    (
        {**page(1), "lines": [{**line(1), "seq": 2**63}]},
        f"the `seq` of a line of job {JOB}'s log is beyond the integers the protocol holds",
    ),
    (
        {**page(1), "lines": [{**line(1), "at_ms": "now"}]},
        f"the `at_ms` of a line of job {JOB}'s log is not an integer",
    ),
    (
        {**page(1), "lines": [{**line(1), "level": None}]},
        f"the `level` of a line of job {JOB}'s log is not a string",
    ),
    (
        {
            **page(1),
            "lines": [{name: value for name, value in line(1).items() if name != "message"}],
        },
        f"a line of job {JOB}'s log has no `message`",
    ),
    (
        {**page(1), "lines": [line(1, dropped_bytes=-3)]},
        f"the `dropped_bytes` of a line of job {JOB}'s log is negative",
    ),
    (
        {**page(1), "lines": [line(1, dropped_bytes="3")]},
        f"the `dropped_bytes` of a line of job {JOB}'s log is not an integer",
    ),
    ({**page(1), "dropped_lines": -1}, f"the `dropped_lines` of job {JOB}'s log is negative"),
    ({**page(1), "next_after": -1}, f"the `next_after` of job {JOB}'s log is negative"),
    ({**page(1), "next_after": 1.0}, f"the `next_after` of job {JOB}'s log is not an integer"),
    ({**page(1), "complete": 1}, f"the `complete` of job {JOB}'s log is not true or false"),
    ({**page(1), "progress": [1, 2]}, f"the `progress` of job {JOB}'s log is not an object"),
    (
        {**page(1), "progress": {"bars_done": 1}},
        f"the progress of job {JOB}'s log has no `bars_total`",
    ),
    (
        {**page(1), "progress": {"bars_done": -1, "bars_total": 1}},
        f"the `bars_done` of the progress of job {JOB}'s log is negative",
    ),
] + [
    (
        {name: value for name, value in page(1).items() if name != missing},
        f"job {JOB}'s log has no `{missing}`"
        if missing != "job_id"
        else "a page of a job's log has no `job_id`",
    )
    for missing in ("job_id", "lines", "dropped_lines", "next_after", "complete")
]


@pytest.mark.parametrize(("answered", "why"), UNREADABLE_PAGES)
def test_a_page_the_protocol_does_not_write_is_refused_and_never_guessed_at(
    server, job, answered, why
):
    server.says("GET", f"{LOGS}?after=0", (200, answered))
    error = raised(qf.RemoteError, lambda: list(job.stream_logs()))
    assert str(error) == (
        f"the server at {server.url}/ answered GET {LOGS}?after=0 with a body the protocol does "
        f"not read: {why}; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)


def test_a_page_is_read_past_what_a_later_revision_adds_and_takes_a_null_for_nothing(server, job):
    wider = page(1, complete=True, progress=None, checkpoint={"bars": 5})
    wider["progress"] = None
    wider["lines"][0].update(dropped_bytes=None, source="stdout")
    server.says("GET", f"{LOGS}?after=0", (200, wider))
    (only,) = job.stream_logs()
    assert (only.seq, only.dropped_bytes) == (1, None)


def test_what_a_server_refuses_a_log_with_is_raised_from_the_loop_in_its_words(server, job):
    # The stream is refused as the route is, while it is still HTTP, and
    # the page that is asked for next says why.
    missing = refusal(404, "not_found", f"the server holds no job {JOB}")
    server.says("GET", f"{LOGS}?after=0", missing)
    logs = job.stream_logs()
    error = raised(qf.NotFound, lambda: next(logs))
    assert (str(error), error.code, error.status) == (
        f"the server holds no job {JOB}",
        "not_found",
        404,
    )
    assert routes(server) == ["/logs?after=0 (stream)", "/logs?after=0"]
    # A log that was followed for a while fails where it stopped, and is
    # followed again from the last line given.
    server.streams(f"{LOGS}?after=0", a_line(1), a_line(2), closing(1001))
    server.says(
        "GET", f"{LOGS}?after=2", refusal(500, "internal", "the server's store failed to answer")
    )
    given = []
    error = raised(qf.RemoteError, lambda: given.extend(job.stream_logs()))
    assert numbers(given) == [1, 2]
    assert (str(error), error.code) == ("the server's store failed to answer", "internal")
    server.says("GET", f"{LOGS}?after=2", (200, page(3, complete=True)))
    assert numbers(job.stream_logs(after=given[-1].seq)) == [3]


def test_a_server_that_is_gone_is_unavailable_from_the_loop(job, server):
    port = server.port
    server.close()
    error = raised(qf.ServerUnavailable, lambda: list(job.stream_logs()))
    assert str(error).startswith(f"could not reach the server at http://127.0.0.1:{port}/: ")


def test_a_server_that_speaks_no_version_the_package_does_has_no_log_to_follow():
    with StandIn(speaks={"version": "9.0.0", "proto_version": 9, "min_supported": 8}) as later:
        client = qf.Client(later.url)
        # A job cannot be had of such a server; a handle made before it was
        # upgraded would ask it again.
        with StandIn() as earlier:
            earlier.says("GET", f"/v1/backtests/{JOB}", (200, written()))
            held = qf.Client(earlier.url).backtest.get(JOB)
        raised(qf.VersionMismatch, lambda: client.backtest.get(JOB))
        assert later.routes == ["GET /version"]
        del held


def test_a_token_over_http_goes_on_no_stream_that_does_not_end_on_this_machine(monkeypatch):
    # `localhost` leads where the system's resolver says. A stream is held
    # to this machine before anything is sent on it, as a request is, and
    # the request that follows says where the name led.
    with StandIn() as server:
        server.says("GET", f"/v1/backtests/{JOB}", (200, written(state=RUNNING)))
        server.streams(f"{LOGS}?after=0", a_line(1), said("end"), closing())
        client = qf.Client(server.url.replace("127.0.0.1", "localhost"), token=TOKEN)
        import socket

        real = socket.getaddrinfo

        def led(host, *arguments, **named):
            return real("127.0.0.1" if host == "localhost" else host, *arguments, **named)

        monkeypatch.setattr(socket, "getaddrinfo", led)
        job = client.backtest.get(JOB)
        before = len(server.asked)
        monkeypatch.setattr(_wire, "_loopback", lambda address: False)
        error = raised(qf.RemoteError, lambda: list(job.stream_logs()))
        assert "is not on this machine" in str(error) and "nothing was sent" in str(error)
        # Nothing was: neither the upgrade nor the page.
        assert len(server.asked) == before
        assert TOKEN not in str(error)


# A line.


def test_a_line_is_a_value_that_is_never_built_changed_or_derived_from(server, job, refused):
    server.streams(
        f"{LOGS}?after=0",
        a_line(42, message="fast crossed above slow at 81940.01"),
        a_line(42, message="fast crossed above slow at 81940.01"),
        said("end"),
        closing(),
    )
    (first,) = job.stream_logs()
    (again,) = job.stream_logs()
    assert repr(first) == "<LogLine 42 info: 'fast crossed above slow at 81940.01'>"
    assert first == again and first is not again and not first != again
    refused(
        TypeError,
        "a LogLine cannot be built: Job.stream_logs gives one for each line of a job's log",
        qf.LogLine,
    )
    refused(
        TypeError,
        "a LogLine cannot be built: Job.stream_logs gives one for each line of a job's log",
        lambda: qf.LogLine(seq=1, at_ms=1, level="info", message="m", dropped_bytes=None),
    )
    refused(
        AttributeError,
        "attribute 'message' of 'quantforge_py.LogLine' objects is not writable",
        setattr,
        first,
        "message",
        "another",
    )
    refused(
        AttributeError,
        "'quantforge_py.LogLine' object has no attribute 'text'",
        setattr,
        first,
        "text",
        "another",
    )
    refused(
        AttributeError,
        "attribute 'seq' of 'quantforge_py.LogLine' objects is not writable",
        delattr,
        first,
        "seq",
    )
    assert not hasattr(first, "__dict__")
    # A record, as the engine's are: equal by its fields, and no key.
    with pytest.raises(TypeError, match="unhashable type"):
        hash(first)
    for copied in (copy.copy, copy.deepcopy, pickle.dumps):
        refused(TypeError, "cannot pickle 'quantforge_py.LogLine' object", copied, first)
    with pytest.raises(TypeError, match="not an acceptable base type"):
        type("Derived", (qf.LogLine,), {})
    assert first != (42, "info") and first != None  # noqa: E711


def test_two_lines_are_equal_when_every_field_is(server, job):
    server.streams(
        f"{LOGS}?after=0",
        a_line(1),
        a_line(2),
        said("line", **{**line(3), "at_ms": 7}),
        said("line", **{**line(4), "level": "error"}),
        said("line", **{**line(5), "message": "another"}),
        a_line(6, dropped_bytes=3),
        said("end"),
        closing(),
    )
    server.streams(f"{LOGS}?after=0", *[a_line(seq) for seq in range(1, 7)], said("end"))
    changed = list(job.stream_logs())
    plain = list(job.stream_logs())
    assert [one == other for one, other in zip(changed, plain)] == [True, True] + [False] * 4


def test_a_line_whose_text_is_long_or_holds_control_characters_is_written_safely(server, job):
    server.streams(
        f"{LOGS}?after=0",
        a_line(1, message="\x1b[2J\r\nrm -rf"),
        a_line(2, message="x" * 500),
        said("end"),
        closing(),
    )
    escaped, long = job.stream_logs()
    assert repr(escaped) == r"<LogLine 1 info: '\x1b[2J\r\nrm -rf'>"
    assert repr(long) == f"<LogLine 2 info: {'x' * 80!r}...>"
    # The text itself is the job's own, with every character it wrote.
    assert escaped.message == "\x1b[2J\r\nrm -rf" and len(long.message) == 500
