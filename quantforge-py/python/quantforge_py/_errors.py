"""What a request to a QuantForge server fails with: `RemoteError`, and the
six exceptions derived from it, which the package publishes
(`docs/python-client.md`).

Each is told by what a caller does about it. One a server refused a request
with is raised by the code of the protocol's error envelope, and says what
the server said, word for word.

This module is private: import its names from `quantforge_py`.
"""


def _published(cls):
    """Names `cls` as the package publishes it: a class says where it is
    imported from, as the extension module's classes do."""
    cls.__module__ = __name__.rpartition(".")[0]
    return cls


@_published
class RemoteError(Exception):
    """A request to a QuantForge server that did not get its answer.

    Every failure of a request is this exception or one derived from it.
    `str()` of one a server refused a request with is the server's own
    message, word for word.

    - `code`: the protocol's code for a refusal, as the server wrote it, such
      as `"invalid_request"`; `None` where no QuantForge server refused the
      request: no answer came, or one came that is not QuantForge's.
    - `status`: the HTTP status the request was refused with; `None` where no
      answer came, or the answer was no refusal.

    A code that has no exception of its own, `internal` among them, is raised
    as `RemoteError` itself.
    """

    def __init__(self, message, *, code=None, status=None):
        super().__init__(message)
        self.code = code
        self.status = status


@_published
class AuthError(RemoteError):
    """The server refused the request's credentials: the token is missing,
    unknown or expired. The protocol's `unauthorized`, HTTP 401; and a 401 or
    a 403 that something in front of the server answered in its place."""


@_published
class ValidationError(RemoteError):
    """The server refused the request as malformed or as naming something
    invalid, and sending it again unchanged fails again. The protocol's
    `invalid_request`, HTTP 400, with the server's message word for word."""


@_published
class NotFound(RemoteError):
    """What the request names does not exist on the server, such as a job of
    that ID. The protocol's `not_found`, HTTP 404."""


@_published
class Conflict(RemoteError):
    """The request contradicts the state of what it names, such as cancelling
    a job that has ended. The protocol's `conflict`, HTTP 409."""


@_published
class ServerUnavailable(RemoteError):
    """The server could not be reached, or cannot answer now: no connection,
    one that broke or timed out, the protocol's `unavailable`, HTTP 503, once
    it was asked again a few times, or a 502, 503 or 504 that something in
    front of the server answered in its place. The request may be made
    again."""


@_published
class VersionMismatch(RemoteError):
    """The server and this package share no protocol version: found as the
    client first asks the server which versions it speaks, or answered by a
    server as the protocol's `unsupported_version`, HTTP 400.

    - `supported`: the versions the server speaks, the oldest and the newest,
      as a tuple of two `int`s; `None` where it did not say.
    """

    def __init__(self, message, *, code=None, status=None, supported=None):
        super().__init__(message, code=code, status=status)
        self.supported = supported


# The exception a refusal is raised as, by the protocol's code. Any other
# code, one a later revision of the protocol adds among them, is a
# `RemoteError`.
_RAISED = {
    "invalid_request": ValidationError,
    "unauthorized": AuthError,
    "not_found": NotFound,
    "conflict": Conflict,
    "unsupported_version": VersionMismatch,
    "unavailable": ServerUnavailable,
}

# The exception an answer without the protocol's envelope is raised as, by
# its HTTP status: what a proxy in front of a server answers in its place.
_RAISED_BY_STATUS = {
    401: AuthError,
    403: AuthError,
    502: ServerUnavailable,
    503: ServerUnavailable,
    504: ServerUnavailable,
}
