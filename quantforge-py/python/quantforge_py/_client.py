"""The client of a QuantForge server: `Client`, which the package publishes
(`docs/python-client.md`).

A client is where its server is, the token it sends, and the server's
backtest jobs. What reaches the server is `quantforge_py._wire`, what reads
its answer `quantforge_py._answer`, the jobs are `quantforge_py._jobs`, and
what a request fails with is `quantforge_py._errors`.

This module is private: import its names from `quantforge_py`.
"""

from ._errors import _published
from ._jobs import _Backtests, _final, _not_writable
from ._wire import _Server, _token, _Wire


@_final
@_published
class Client:
    """The client of a QuantForge server, `quantforge serve run`, that
    answers at `base_url`.

    `Client(base_url, token=None)` asks the server nothing: it is first
    asked when the client is first used.

    - `base_url`: where the server answers, `http://` or `https://` with a
      host, such as `http://127.0.0.1:9600`, optionally under a path.
    - `token`: sent with every request, as `Authorization: Bearer`, and
      never shown. Left out, it is read from `QF_REMOTE_TOKEN`; an empty one
      is none, and never falls back to the variable. A token goes only over
      `https`, or over `http` to this machine.

    What a client holds:

    - `url`: the server's URL, as the client asks it.
    - `backtest`: the server's backtest jobs: `backtest.list()` and
      `backtest.get(job_id)`, each of which gives a `Job`.
    - `server_version` and `proto_version`: the server's release and the
      protocol version agreed with it; `None` until the server has been
      asked.
    """

    __slots__ = ("_wire", "_backtest", "_sends")

    def __init__(self, base_url, token=None):
        server = _Server(base_url)
        token, source = _token(token)
        if token is not None and not (server.https or server.local):
            refusal = (
                "a token goes only over https, or over http to this machine, and the server at "
                f"{server.shown} is neither; nothing was sent"
            )
            if source != "token":
                # A token nobody gave this client: say where it came from,
                # and how a client is made without it.
                refusal = f'{refusal}. The token is {source}\'s: pass token="" to send none'
            raise ValueError(refusal)
        wire = _Wire(server, token)
        object.__setattr__(self, "_wire", wire)
        object.__setattr__(self, "_backtest", _Backtests(wire))
        object.__setattr__(self, "_sends", token is not None)

    def __setattr__(self, name, value):
        _not_writable(self, name)

    def __delattr__(self, name):
        _not_writable(self, name)

    @property
    def url(self):
        """The server's URL, as the client asks it."""
        return self._wire.server.shown

    @property
    def backtest(self):
        """The server's backtest jobs: `list()` and `get(job_id)`."""
        return self._backtest

    @property
    def server_version(self):
        """The server's release, as it names it, such as `0.3.0`; `None`
        until the server has been asked."""
        agreed = self._wire.agreed
        return None if agreed is None else agreed[0]

    @property
    def proto_version(self):
        """The protocol version agreed with the server, the newest both
        sides speak; `None` until the server has been asked."""
        agreed = self._wire.agreed
        return None if agreed is None else agreed[1]

    def handshake(self):
        """Asks the server which protocol versions it speaks, and agrees on
        the newest both sides do: what a client's first use does, done now.
        A server that was asked is not asked again.

        A server that cannot be reached is a `ServerUnavailable`, and one
        that speaks no version this package speaks a `VersionMismatch`.
        """
        self._wire.handshake()

    def __repr__(self):
        token = ", with a token" if self._sends else ""
        return f"<Client {self.url}{token}>"
