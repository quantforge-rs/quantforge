"""A job's log as a client follows it: `LogLine`, which the package
publishes, and the log read to its end, over the stream a server offers
and a page at a time wherever the stream cannot be had
(`docs/python-jobs.md`).

A server numbers a job's lines from 1 as it takes them, so a line's number
is a cursor: what follows line `seq` is asked for with `after=seq`. A log is
followed by that cursor alone, whichever way it is read:

- **The stream first.** `GET /v1/backtests/{job_id}/logs` asked as a
  WebSocket sends the lines the server holds and then each new one, how far
  the job has come, and `end` once the job has ended and its last line was
  sent (`quantforge_py._websocket`).
- **Pages wherever the stream stops.** A stream that cannot be opened, that
  breaks, falls silent, says something the protocol does not write, or
  closes without `end`, is given up for the same route asked plainly, from
  the cursor: a page of lines at a time, until one says the log is
  complete.
- **No line twice, and none left out.** A line at or before the cursor is
  passed over, and lines the server no longer holds move the cursor past
  them, as the server numbers its next line past them.
- **A failure is raised by the pages.** Nothing a stream stops with is
  raised: what is wrong with a server shows as the pages ask it, as the
  exception every request raises for it. A stream that may not be asked
  for at all, one that would carry a token over `http` to another machine,
  is refused as a request is, before anything is sent.

This module is private: import its names from `quantforge_py`.
"""

import time

from ._errors import _published
from ._jobs import _count, _final, _given, _is_job_id, _Record
from ._wire import (
    _MOST_INTEGER,
    _member,
    _object,
    _optional,
    _parsed,
    _quoted,
    _Unreadable,
    _wrong_type,
)

# How long a follower waits before it asks for the next page, where the
# last one moved the cursor no further, in seconds.
_PAGE_SECONDS = 0.5


@_final
@_published
class LogLine(_Record):
    """One line of a job's log, as its server holds it.

    `Job.stream_logs` gives one for each line, and none is built. It is a
    value, and never changes: two are equal when every field is.

    - `seq`: the line's number in its job's log. A server numbers a job's
      lines from 1, in the order it took them: `after=seq` follows the log
      from the line after this one.
    - `at_ms`: when the server took the line, in milliseconds since the
      Unix epoch, UTC, by the server's clock.
    - `level`: how serious the line is: `"debug"`, `"info"`, `"warning"`
      or `"error"`. What a strategy prints is `"info"`, and what it writes
      to its standard error `"warning"`. A level a later revision of the
      protocol adds is given as the server wrote it.
    - `message`: the line's text, as the job wrote it.
    - `dropped_bytes`: how many bytes of the text the server left out,
      where the line was longer than it keeps of one, so that `message` is
      the line's start; `None` for a line that is whole.
    """

    __slots__ = ("_seq", "_at_ms", "_level", "_message", "_dropped_bytes")

    _BUILT = "a LogLine cannot be built: Job.stream_logs gives one for each line of a job's log"

    seq = _given("_seq", "The line's number in its job's log, from 1.")
    at_ms = _given("_at_ms", "When the server took the line, by its own clock.")
    level = _given("_level", "How serious the line is: debug, info, warning or error.")
    message = _given("_message", "The line's text, as the job wrote it.")
    dropped_bytes = _given(
        "_dropped_bytes", "How many bytes of the text the server left out, or None."
    )

    def __repr__(self):
        return f"<LogLine {self._seq} {self._level}: {_quoted(self._message)}>"


def _asked(after, on_progress):
    """Where a log is followed from and who is told how far its job has
    come, as a caller names them, checked before anything is asked."""
    if isinstance(after, bool) or not isinstance(after, int):
        raise _wrong_type("after", "an int", after)
    # Told apart before either is written: Python refuses to write a number
    # of thousands of digits.
    if after < 0:
        raise ValueError(
            "after is negative: it is the number of the last line the caller holds, or 0 for "
            "the log from its first line"
        )
    if after >= _MOST_INTEGER:
        raise ValueError("after is beyond the number any line of a log has")
    if on_progress is not None and not callable(on_progress):
        raise _wrong_type("on_progress", "a callable or None", on_progress)
    return after, on_progress


def _line(record, what):
    """One line of a log as the protocol writes it, read strictly."""
    seq = _member(record, "seq", int, what)
    if seq < 1:
        raise _Unreadable(f"the `seq` of {what} is no line's number, which is 1 or more")
    dropped = _optional(record, "dropped_bytes", int, what)
    if dropped is not None and dropped < 0:
        raise _Unreadable(f"the `dropped_bytes` of {what} is negative")
    return LogLine._of(
        seq=seq,
        at_ms=_member(record, "at_ms", int, what),
        level=_member(record, "level", str, what),
        message=_member(record, "message", str, what),
        dropped_bytes=dropped,
    )


def _progress(record, what):
    """How far a job has come as the protocol writes it: the bars it has
    run, and the bars it runs on."""
    return _count(record, "bars_done", what), _count(record, "bars_total", what)


def _page(job_id):
    """Reads the answer of a log's route asked plainly: one page of the log
    of the job `job_id`, and of no other."""

    def read(value):
        what = "a page of a job's log"
        record = _object(value, what)
        named = _member(record, "job_id", str, what)
        if not _is_job_id(named):
            raise _Unreadable(f"the `job_id` of {what} is no job ID")
        if named != job_id:
            raise _Unreadable(f"it is a page of job {named}'s log, not of job {job_id}'s")
        what = f"job {job_id}'s log"
        lines = [
            _line(_object(each, f"a line of {what}"), f"a line of {what}")
            for each in _member(record, "lines", list, what)
        ]
        dropped = _count(record, "dropped_lines", what)
        after = _count(record, "next_after", what)
        told = _optional(record, "progress", dict, what)
        progress = None if told is None else _progress(told, f"the progress of {what}")
        return lines, dropped, after, progress, _member(record, "complete", bool, what)

    return read


def _frame(message):
    """One frame of a log's stream as the protocol writes it: its type, and
    what it holds. A type a later revision of the protocol adds holds
    nothing here."""
    what = "a frame of a job's log"
    record = _object(_parsed(message.encode("utf-8")), what)
    kind = _member(record, "type", str, what)
    if kind == "line":
        return kind, _line(record, "a line of a job's log")
    if kind == "progress":
        return kind, _progress(record, "the progress of a job's log")
    if kind == "truncated":
        return kind, _count(record, "dropped_lines", what)
    return kind, None


class _Cursor:
    """Where a follower stands in a log: the number of the last line it
    accounts for, and how far it last said the job has come."""

    __slots__ = ("after", "_on_progress", "_told")

    def __init__(self, after, on_progress):
        self.after = after
        self._on_progress = on_progress
        self._told = None

    def passes(self, line):
        """Whether a line is one to give: one after the cursor, which moves
        to it."""
        if line.seq <= self.after:
            return False
        self.after = line.seq
        return True

    def dropped(self, count):
        """Lines the server no longer holds, right after the cursor: the
        server numbers its next line past them, so the cursor passes them
        too, and they are not counted again where the log is asked for
        anew."""
        self.after += count

    def told(self, progress):
        """Tells the caller how far the job has come, where that is not
        what it was last told."""
        if progress != self._told:
            self._told = progress
            if self._on_progress is not None:
                self._on_progress(*progress)


def _streamed(wire, job_id, cursor):
    """Follows a log over its stream, from the cursor: gives each line, and
    answers whether the stream said the log has ended. One that stopped any
    other way answers `False`, with the cursor where it stopped."""
    import http.client

    from . import _answer, _websocket

    # Whatever stops a stream, a failure of its connection or something it
    # said that the protocol does not write: none of it is raised.
    stops = (OSError, http.client.HTTPException, _answer._NotHTTP, _websocket._Broken, _Unreadable)
    try:
        stream = wire.stream(f"backtests/{job_id}/logs?after={cursor.after}")
    except stops:
        return False
    try:
        while True:
            try:
                message = stream.message()
                kind, held = (None, None) if message is None else _frame(message)
            except stops:
                return False
            if kind == "line":
                if cursor.passes(held):
                    yield held
            elif kind == "progress":
                cursor.told(held)
            elif kind == "truncated":
                cursor.dropped(held)
            elif kind == "end":
                return True
            elif kind is None or kind == "error":
                # Closed without `end`, or with what stopped the server:
                # the pages say what there is to say of it.
                return False
    finally:
        stream.close()


def _paged(wire, job_id, cursor):
    """Follows a log a page at a time, from the cursor, to the page that
    says the log is complete: gives each line."""
    read = _page(job_id)
    while True:
        before = cursor.after
        route = f"backtests/{job_id}/logs?after={before}"
        lines, dropped, after, progress, complete = wire.ask("GET", route, read)
        cursor.dropped(dropped)
        for line in lines:
            if cursor.passes(line):
                yield line
        if progress is not None:
            cursor.told(progress)
        cursor.after = max(cursor.after, after)
        if complete:
            return
        if cursor.after == before:
            # Nothing new: the job is quiet, or has not started.
            time.sleep(_PAGE_SECONDS)


def _followed(wire, job_id, after, on_progress):
    """A job's log from the line after `after` to its end, line by line:
    over the stream, and a page at a time from wherever the stream
    stopped."""
    cursor = _Cursor(after, on_progress)
    # Asked here, where what it fails with is raised: a server that speaks
    # no version the package does has no log to follow either way.
    wire.handshake()
    ended = yield from _streamed(wire, job_id, cursor)
    if not ended:
        yield from _paged(wire, job_id, cursor)
