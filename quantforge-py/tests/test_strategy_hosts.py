"""The threads a strategy's callbacks run on, and the interpreter they hold.

A callback attaches its thread to Python for as long as it runs. Hosted by
Python, a run is on the thread that asked for it, which is attached
already. A host that embeds Python runs the engine on a thread of its own,
which it attaches once around the whole run: the bench's `embedded` runs do
the same, so what holds for them holds for such a host.

A test that waits on another thread waits with a limit. One that stopped
for good would be ended by the run's own limit (`faulthandler_timeout` in
`pyproject.toml`), with every thread's traceback."""

import decimal
import signal
import sys
import threading

import pytest

import quantforge_py as qf

# How long a wait on another thread may take before the test fails.
PATIENCE = 30

# A switch interval, in seconds, longer than a test is given
# (`faulthandler_timeout`). Python asks the thread that holds the interpreter
# to give it up only once another has waited that long, so while this is
# set a thread that waits is handed the interpreter only where the one that
# holds it lets go, however slowly a test runs.
NO_SWITCH = 600.0


class Watches(qf.Strategy):
    """Writes down the thread of every callback, and what that thread says
    of its own state."""

    def __init__(self):
        self.threads = []
        self.own = threading.local()
        self.seen = []

    def look(self):
        self.threads.append(threading.get_ident())
        self.seen.append((decimal.getcontext().prec, getattr(self.own, "mark", None)))

    def on_start(self, ctx):
        self.look()
        # State of the thread, not of the strategy.
        decimal.getcontext().prec = 41
        self.own.mark = "set in on_start"

    def on_bar(self, ctx, bar):
        self.look()

    def on_finish(self, ctx):
        self.look()


def test_hosted_by_python_every_callback_runs_on_the_thread_that_asked(bench, market, minutes):
    strategy = Watches()
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with decimal.localcontext():
        bench(market, minutes(*[(100, 100)] * 5), strategy, config)
        # The thread is the caller's, and so is its state: what a callback
        # changed of it, the caller finds changed.
        assert decimal.getcontext().prec == 41
    assert strategy.threads == [threading.get_ident()] * 7
    assert strategy.own.mark == "set in on_start"


def test_hosted_by_python_a_run_keeps_the_interpreter_between_its_callbacks(
    bench, market, minutes, pytestconfig
):
    # The limit of a test is what keeps the interval from running out in one.
    assert float(pytestconfig.getini("faulthandler_timeout")) < NO_SWITCH
    ready, handed, stop = threading.Event(), threading.Event(), threading.Event()

    def wait_for_the_interpreter():
        ready.wait(PATIENCE)
        # Here this thread has the interpreter. It says so and lets go of it
        # at once, so that a run that handed it over goes on, and fails.
        handed.set()
        stop.wait(PATIENCE)

    class Looks(qf.Strategy):
        def __init__(self):
            self.bars = 0

        def on_bar(self, ctx, bar):
            self.bars += 1
            if handed.is_set():
                raise AssertionError(f"the interpreter was given up by bar {self.bars}")

    # Enough bars that a thread waiting for the interpreter would take it at
    # one of them, were it given up after every callback: it is taken at
    # about one such chance in fifteen.
    bars = 10_000
    strategy = Looks()
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    candles = minutes(*[(100, 100)] * bars)
    waiting = threading.Thread(target=wait_for_the_interpreter, daemon=True)
    interval = sys.getswitchinterval()
    # Set before the other thread starts: it never waits for the interpreter
    # under a shorter interval, which could run out during the run.
    sys.setswitchinterval(NO_SWITCH)
    try:
        waiting.start()
        # From here the other thread wants the interpreter, and is handed it
        # only if the run lets go of it.
        ready.set()
        bench(market, candles, strategy, config)
        during = handed.is_set()
    finally:
        sys.setswitchinterval(interval)
        # It was waiting all along: it is handed the interpreter as soon as
        # this thread waits for it, and is stopped whatever became of the
        # run.
        ran = handed.wait(PATIENCE)
        stop.set()
        waiting.join(PATIENCE)
    assert ran and not waiting.is_alive()
    assert strategy.bars == bars
    assert not during


def test_embedded_every_callback_runs_on_one_thread_of_the_engines_own(bench, market, minutes):
    strategy = Watches()
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with decimal.localcontext() as mine:
        mine.prec = 17
        bench(market, minutes(*[(100, 100)] * 5), strategy, config, embedded=True)
        # Nothing of the calling thread's state was read or changed.
        assert decimal.getcontext().prec == 17
    assert len(strategy.threads) == 7
    assert len(set(strategy.threads)) == 1
    assert strategy.threads[0] != threading.get_ident()
    assert not hasattr(strategy.own, "mark")
    # The engine's thread started with a state of its own, not the caller's.
    assert strategy.seen[0] == (decimal.DefaultContext.prec, None)


def test_a_threads_own_state_lasts_from_one_callback_to_the_next(run):
    strategy = Watches()
    with decimal.localcontext():
        decimal.getcontext().prec = decimal.DefaultContext.prec
        run(strategy)
    # What on_start set is there on every bar and at the finish: the
    # precision a strategy computes with does not change under it.
    assert strategy.seen == [(decimal.DefaultContext.prec, None)] + [(41, "set in on_start")] * 6


def test_another_thread_runs_while_a_callback_waits_for_it(run):
    asked, answered = threading.Event(), threading.Event()

    def answer():
        if asked.wait(PATIENCE):
            answered.set()

    class Asks(qf.Strategy):
        def __init__(self):
            self.answers = []

        def on_bar(self, ctx, bar):
            asked.set()
            # Waiting gives the interpreter up, and the callback has it
            # back when the wait is over.
            self.answers.append(answered.wait(PATIENCE))

    other = threading.Thread(target=answer)
    other.start()
    strategy = Asks()
    run(strategy)
    other.join(PATIENCE)
    assert not other.is_alive()
    assert strategy.answers == [True] * 5


def test_a_callback_starts_and_joins_a_thread_of_its_own(run):
    class Delegates(qf.Strategy):
        def __init__(self):
            self.computed = []

        def on_bar(self, ctx, bar):
            worker = threading.Thread(target=lambda: self.computed.append(bar.open_time_ms))
            worker.start()
            worker.join(PATIENCE)
            assert not worker.is_alive()

    strategy = Delegates()
    run(strategy)
    assert strategy.computed == [0, 60_000, 120_000, 180_000, 240_000]


@pytest.mark.parametrize(
    "inner_embedded", [False, True], ids=["inner hosted by python", "inner embedded"]
)
def test_a_callback_runs_a_backtest_of_its_own(run, bench, market, minutes, inner_embedded):
    candles = minutes((100, 100), (100, 110), (110, 120), (125, 125))
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

    class Enters(qf.Strategy):
        def on_bar(self, ctx, bar):
            return qf.TargetPosition.LONG_ALL_IN

    class Nests(qf.Strategy):
        def __init__(self):
            self.inner = []

        def on_bar(self, ctx, bar):
            self.inner.append(bench(market, candles, Enters(), config, embedded=inner_embedded))
            return qf.TargetPosition.LONG_ALL_IN

    strategy = Nests()
    outer = run(strategy, candles)
    alone = bench(market, candles, Enters(), config)
    # Each run is its own: the inner ones change nothing of the outer.
    assert strategy.inner == [alone] * 4
    assert outer == alone
    assert outer.final_equity == 1250


def test_runs_on_several_threads_at_once_stay_apart(bench, market, minutes, embedded):
    candles = minutes(*[(100 + index % 7, 101 + index % 5) for index in range(200)])
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=10, close_out_at_end=True)

    class Alternates(qf.Strategy):
        def __init__(self, every):
            self.every, self.bars = every, 0

        def on_bar(self, ctx, bar):
            self.bars += 1
            if self.bars % self.every:
                return None
            holds = ctx.position_qty > 0
            return qf.TargetPosition.FLAT if holds else qf.TargetPosition.LONG_ALL_IN

    periods = [2, 3, 5, 7]
    alone = [bench(market, candles, Alternates(every), config) for every in periods]
    together = [None] * len(periods)
    started = threading.Barrier(len(periods), timeout=PATIENCE)

    def one(index):
        started.wait()
        strategy = Alternates(periods[index])
        together[index] = bench(market, candles, strategy, config, embedded=embedded)

    threads = [threading.Thread(target=one, args=(index,)) for index in range(len(periods))]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(PATIENCE)
    assert not any(thread.is_alive() for thread in threads)
    assert together == alone
    assert len({result.final_equity for result in alone}) == len(periods)


def test_an_interrupt_delivered_during_a_run_stops_it_at_the_callback(bench, market, minutes):
    if threading.current_thread() is not threading.main_thread():
        pytest.skip("Python delivers a signal to its main thread")

    class Interrupted(qf.Strategy):
        def __init__(self):
            self.bars = 0

        def on_bar(self, ctx, bar):
            self.bars += 1
            if self.bars == 3:
                # As Ctrl-C does: Python raises it in the code it is running.
                signal.raise_signal(signal.SIGINT)
            # A loop, where Python looks for a signal that has come in.
            for _ in range(1_000):
                pass

    strategy = Interrupted()
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    # Python's own handler, for the length of the test: a process started
    # with the signal ignored, as a shell starts a background job, has none.
    before = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        with pytest.raises(KeyboardInterrupt) as caught:
            bench(market, minutes(*[(100, 100)] * 50), strategy, config)
    finally:
        if before is not None:
            signal.signal(signal.SIGINT, before)
    assert strategy.bars == 3
    assert caught.value.__notes__ == [
        'the strategy "Interrupted" failed in on_bar, at the bar that opened at 120000'
    ]
