# Automation requirements

This page traces the v0.3 automation's requirements to the stories that
deliver them. Two sources feed it:
- every functional requirement of the automation PRD (iteration 3, 13 pages,
  page 1 being the cover);
- the parity requirement added on 2026-10-21.

Each requirement is mapped to:
- the story that implements it, and the stories that support it or give it
  parity on a second venue;
- the section of [ADR 0002](adr/0002-automation-architecture.md) that decides
  its policy.

The IDs `OBS-R001`–`OBS-R072` and `D01`–`D24` match Linear and the v0.3 work
order. Requirements are paraphrased, not quoted.

## Reading and maintaining it

- **This page plans coverage. It is not evidence.** A requirement is met when
  ALG-769 links passing evidence for every venue, store and product cell it
  touches, and ALG-755 accepts it.
- **Keep it in step with scope.** When a story's scope changes, update this page
  in the same change. Dropping a required venue, store, family or operation
  needs Marko's explicit decision, recorded below and as a scope change in the
  parity matrix.
- **How it relates to the other matrices:**
  - [exchange-feature-matrix.md](exchange-feature-matrix.md) is generated from
    the Spot connector's declared capabilities and conformance checks.
  - The [parity matrix](parity/README.md) turns the rows here into testable
    venue × store × product × operation cells, with owners and evidence. Every
    ID below maps to at least one of its capabilities.
- **The venue facts** these requirements depend on are in
  [futures-venues.md](futures-venues.md).

## Scope decisions

| Date | Decision | By |
| --- | --- | --- |
| 2026-10-21 | Parity: the same functions on Binance and Bybit, SQLite and PostgreSQL (P-001 below) | Marko |
| 2026-10-22 | Crypto perpetuals only. Dated futures and TradFi perpetuals are refused, but both families stay required on both venues (ADR 0002 §3; parity scope change SC-1) | Marko |
| 2026-10-22 | A profile owns only positions it opened; there is no adoption in v0.3 (ADR 0002 §10; parity scope change SC-2) | Marko |
| 2026-10-22 | Stale entry alerts are refused by age and by price (ADR 0002 §9) | Marko |
| 2026-10-22 | This matrix lives in the repository, citing the PRD by section and page | Marko |

## Parity requirement

| ID | Source | Requirement | Primary | Also | ADR 0002 |
| --- | --- | --- | --- | --- | --- |
| P-001 | User requirement, 2026-10-21 | The same authenticated alerts, risk sizing, native stop, monitoring, CLI and Telegram functions on Binance and Bybit, for USDT-settled linear and coin-settled inverse futures, LONG and SHORT, on SQLite and PostgreSQL | ALG-756 | ALG-767, ALG-769 | §13 |

## Functional requirements

| ID | PRD | Requirement | Primary | Also | ADR 0002 |
| --- | --- | --- | --- | --- | --- |
| OBS-R001 | §1 (p.3) | Execution on the PRD's exchange, extended to equivalent Binance functions; similar products' feature catalogues are not imported | ALG-732 | ALG-756 | §13 |
| OBS-R002 | §1 (p.3); §3.1 (p.5); §7 (p.13) | USDT-settled linear and coin-settled inverse futures, with their real contract rules | ALG-739 | ALG-760 | §3, §6 |
| OBS-R003 | §2.1 (p.4); §3.4 (p.7); §7 (p.13) | LONG and SHORT entry, protection, monitoring and close in both families | ALG-740 | ALG-741, ALG-762, ALG-763 | §5, §7 |
| OBS-R004 | §2 (pp.3–4) | Receive TradingView entry alerts and exit or take-profit alerts | ALG-737 | — | §1, §9 |
| OBS-R005 | §2.1–2.2 (p.4) | Validate entry and exit payloads before acting on them | ALG-743 | ALG-737 | §9 |
| OBS-R006 | §7 (p.13) | Authenticate incoming webhooks | ALG-743 | ALG-754 | §15 |
| OBS-R007 | §7 (p.13) | A duplicated signal never opens another position | ALG-738 | ALG-745, ALG-767 | §5, §9 |
| OBS-R008 | §2.1 step 2 (p.4) | Identify the instrument and the direction | ALG-737 | ALG-767 | §2, §9 |
| OBS-R009 | §2.1 step 3 (p.4) | Load the asset's risk settings before sizing | ALG-745 | ALG-735 | §6 |
| OBS-R010 | §2.1 steps 1–9 (p.4) | Validate, apply the asset policy, resolve the stop, size, fill, place the native stop, read liquidation, notify | ALG-745 | ALG-746, ALG-751 | §1 |
| OBS-R011 | §2.1 step 6 (p.4) | Enter with a market order, without avoidable delay | ALG-740 | ALG-762 | §1, §9 |
| OBS-R012 | §2.2 (pp.4–5) | Match an exit alert to the open position; no preset take-profit order is needed | ALG-747 | — | §9 |
| OBS-R013 | §2.2 step 3 (p.4) | Close with a market order that cannot open or reverse exposure | ALG-740 | ALG-762 | §1, §9 |
| OBS-R014 | §2.2 (p.5) | No unnecessary or duplicate close, including after a stop loss closed the position | ALG-747 | — | §9, §10 |
| OBS-R015 | §2.2 step 4 (p.4) | Confirm the close on the exchange before announcing it | ALG-747 | — | §1, §12 |
| OBS-R016 | §2.2 steps 5–6 (p.4) | Retrieve the realized PnL and report the outcome | ALG-747 | ALG-751 | §12 |
| OBS-R017 | §3 (p.5) | Size from risk capital set in the CLI, in an explicit currency | ALG-735 | ALG-750 | §6 |
| OBS-R018 | §3.1 (p.5) | The risk budget is the configured capital times the risk percentage | ALG-736 | — | §6 |
| OBS-R019 | §3 (p.5) | Size from the alert's expected entry, and report the actual fill separately | ALG-745 | ALG-736, ALG-751 | §6 |
| OBS-R020 | §3 (p.5) | Sizing considers capital, risk percentage, expected entry, stop, direction, leverage and contract family | ALG-736 | — | §6 |
| OBS-R021 | §3.1 (p.5) | Linear quantity is the risk budget over the distance from entry to stop | ALG-736 | — | §6 |
| OBS-R022 | §3.1 (p.5) | Inverse contract units and their sizing arithmetic | ALG-736 | ALG-760 | §6 |
| OBS-R023 | §3.1 (p.5) | Round quantities to the exchange's increments and minimums | ALG-736 | — | §6 |
| OBS-R024 | §2.1 (p.4); §3.2 (pp.5–6) | Accept an absolute stop price in the entry alert | ALG-736 | ALG-737 | §6, §9 |
| OBS-R025 | §2.1 (p.4); §3.2 (p.6) | Fall back to a per-asset stop percentage set in the CLI | ALG-736 | ALG-735 | §6 |
| OBS-R026 | §2.1 (p.4) | A valid stop in the alert takes precedence over the percentage | ALG-736 | — | §6 |
| OBS-R027 | §2.1 (p.4) | Refuse an entry that has neither | ALG-736 | — | §6 |
| OBS-R028 | §3.2 (p.6) | Percentage stops are E × (1 − p) for LONG and E × (1 + p) for SHORT | ALG-736 | — | §6 |
| OBS-R029 | §3.3 (p.6) | Persist each asset's pair, contract type, risk, default stop, leverage and enabled flag | ALG-735 | — | §6, §15 |
| OBS-R030 | §3.3 (p.6); §6.3 (p.12) | List the configured assets and their settings | ALG-750 | — | — |
| OBS-R031 | §3.3 (p.7); §6.3 (p.12) | Show one asset's effective settings | ALG-750 | — | — |
| OBS-R032 | §3.3 (p.6); §6.3 (p.12) | Add an asset | ALG-750 | — | — |
| OBS-R033 | §3.3 (p.6); §6.3 (p.12) | Edit an asset's risk percentage | ALG-750 | — | §5 |
| OBS-R034 | §3.3 (p.6); §6.3 (p.12) | Edit an asset's default stop percentage | ALG-750 | — | §5 |
| OBS-R035 | §3.3 (p.6); §6.3 (p.12) | Edit an asset's leverage | ALG-750 | — | §4 |
| OBS-R036 | §3.3 (p.7); §6.3 (p.12) | Enable or disable an asset; disabling stops new entries and keeps monitoring, protection and exits | ALG-750 | — | §5 |
| OBS-R037 | §3.3 (p.7) | Default settings for assets without their own | ALG-735 | ALG-750 | §5 |
| OBS-R038 | §3.3 (p.7) | Settings persist across restarts | ALG-735 | — | §15 |
| OBS-R039 | §3.4 (p.7) | Read liquidation from the exchange once the opening is confirmed, in both directions and families | ALG-741 | ALG-763 | §7 |
| OBS-R040 | §3.4 (p.7) | Use the exchange's liquidation value, N/A with a reason when absent; never estimate it or confuse it with the stop | ALG-741 | ALG-763 | §7 |
| OBS-R041 | §3.4 (p.7); §5.2 (p.9); §6.2 (p.12) | Show liquidation in the opened-position message and the CLI position overview | ALG-753 | ALG-751 | §7 |
| OBS-R042 | §3.4 (p.7); §4.2 (p.8) | Refresh liquidation when the position, margin or leverage changes | ALG-744 | ALG-764 | §11 |
| OBS-R043 | §3.4 (p.7) | Warn when the stop would not trigger before liquidation | ALG-746 | — | §7 |
| OBS-R044 | §4.1 (p.7) | Place the exchange-native stop right after opening, at the resolved stop | ALG-746 | ALG-741, ALG-763 | §7 |
| OBS-R045 | §4.1 (p.7); §6.4 (p.13) | The native stop stays active while QuantForge is disconnected or stopped | ALG-741 | ALG-752, ALG-763 | §5, §7 |
| OBS-R046 | §4.1 (p.8) | Report a failed stop placement at once and protect the exposure | ALG-746 | — | §8 |
| OBS-R047 | §4.2 (p.8) | Monitor stop-loss executions | ALG-744 | ALG-764 | §11 |
| OBS-R048 | §4.2 (p.8) | Monitor closes triggered by TradingView | ALG-744 | ALG-764 | §11 |
| OBS-R049 | §4.2 (p.8) | Monitor manual closes on the exchange | ALG-744 | ALG-764 | §10, §11 |
| OBS-R050 | §4.2 (p.8) | Monitor order executions and rejections | ALG-744 | ALG-764 | §11 |
| OBS-R051 | §4.2 (p.8) | Monitor additions to and reductions of a position | ALG-744 | ALG-764 | §10, §11 |
| OBS-R052 | §4.2 (p.8) | Monitor changes to stop orders | ALG-744 | ALG-764 | §10, §11 |
| OBS-R053 | §4.2 (p.8) | Monitor margin and leverage changes and their effect on liquidation | ALG-744 | ALG-764 | §11 |
| OBS-R054 | §4.2 (p.8) | Monitor the exchange connection and surface its loss | ALG-744 | ALG-764 | §11 |
| OBS-R055 | §4.2 (p.8) | Update the position state on every relevant event | ALG-744 | ALG-764 | §11 |
| OBS-R056 | §4.2 (p.8) | Resynchronize with the exchange after a restart or reconnect | ALG-744 | ALG-748, ALG-764 | §11 |
| OBS-R057 | §7 (p.13) | Recover by itself from temporary network interruptions | ALG-744 | ALG-748, ALG-764 | §11 |
| OBS-R058 | §5 (p.8) | Notify a configured Telegram group or channel | ALG-749 | — | §1 |
| OBS-R059 | §5 (p.8); §5.2 (p.9) | Readable messages with consistent icons, telling apart received, executed and closed, LONG and SHORT | ALG-751 | — | — |
| OBS-R060 | §5.1 (pp.8–9) | An entry-received message: asset, direction, signal price, stop price and percentage, processing state | ALG-751 | — | §9 |
| OBS-R061 | §5.2 (p.9) | Announce an opened position only after a confirmed fill and a verified native stop | ALG-751 | — | §1, §7 |
| OBS-R062 | §5.2 (p.9) | An opened message: asset, direction, actual entry, stop and its percentage, liquidation, size, value, margin, leverage, risk and its percentage, protection status | ALG-751 | — | §6, §7 |
| OBS-R063 | §5.3 (pp.9–10) | An indicator-close message: asset, direction, entry, exit, size, realized PnL, return, reason, status | ALG-751 | — | §12 |
| OBS-R064 | §5.4 (p.10) | Detect a stop-loss close and report its actual result, return, reason and status | ALG-751 | ALG-744, ALG-764 | §11, §12 |
| OBS-R065 | §5.5 (pp.10–11) | Error messages (order, stop, payload, margin, API, connection, synchronization, liquidation risk) naming the pair, the error and the position's state | ALG-751 | — | §8, §11 |
| OBS-R066 | §1 (p.3) | A background service run entirely from the CLI, with no GUI | ALG-752 | — | §1 |
| OBS-R067 | §6.1 (p.11) | One lifecycle starts alerts, execution, sizing, stops, liquidation reads, monitoring and Telegram | ALG-752 | — | §1 |
| OBS-R068 | §6.2 (pp.11–12) | Status: running state, exchange connection, balances, counts of open positions and orders, total unrealized PnL | ALG-753 | — | §12 |
| OBS-R069 | §6.2 (p.12) | All 11 position fields: asset, direction, average entry, current price, size, value, margin, leverage, stop and percentage, liquidation, unrealized PnL | ALG-753 | — | §7 |
| OBS-R070 | §6.4 (p.13) | Stop the background service gracefully from the CLI | ALG-752 | — | §5 |
| OBS-R071 | §6.4 (p.13) | A normal stop neither closes positions nor cancels native stops | ALG-752 | — | §5 |
| OBS-R072 | §7 (p.13) | Store exchange credentials securely, protecting both their references and their values | ALG-750 | ALG-754 | §15 |

A dash in the ADR column means the requirement is a presentation or CLI detail
that its story designs within the ADR's rules.

## Decisions

Each decision resolves an ambiguity in the PRD. The decisions are not quotations
from it.

| ID | Topic | Where ADR 0002 leaves it | ADR 0002 | Stories |
| --- | --- | --- | --- | --- |
| D01 | Contracts and the instrument universe | Settled | §3 | ALG-739, ALG-742, ALG-756, ALG-760, ALG-765 |
| D02 | Signal schema and identity | Its rules are settled; ALG-737 writes the schema | §9 | ALG-737, ALG-743, ALG-767 |
| D03 | Webhook authentication | The mechanism is settled; ALG-743 and ALG-754 deploy it | §15 | ALG-743, ALG-754 |
| D04 | The inverse formula and monetary units | Settled | §6 | ALG-733, ALG-736, ALG-742, ALG-760, ALG-765 |
| D05 | Account, position mode and ownership | Settled | §2, §4, §10 | ALG-733, ALG-744, ALG-756, ALG-759, ALG-760, ALG-764, ALG-767 |
| D06 | Duplicate versus new exposure | Settled | §5, §9 | ALG-737, ALG-738, ALG-745, ALG-767 |
| D07 | Defaults and stop precedence | Settled | §5, §6 | ALG-735, ALG-736, ALG-750 |
| D08 | Edits while positions are open or in flight | Settled | §5 | ALG-735, ALG-750, ALG-767 |
| D09 | Partial fills and unknown outcomes | Settled | §7, §9 | ALG-738, ALG-740, ALG-745, ALG-746, ALG-762 |
| D10 | Stop validity and trigger price | Settled: mark price | §6, §7 | ALG-736, ALG-741, ALG-756, ALG-763 |
| D11 | Close races and cycle identity | Settled | §9, §10 | ALG-740, ALG-747, ALG-762 |
| D12 | PnL, fees, funding and return | Settled | §12 | ALG-733, ALG-747, ALG-751, ALG-764 |
| D13 | Configured capital versus exchange balances | Settled | §6 | ALG-735, ALG-736, ALG-753 |
| D14 | Rounding, cost allowance and risk limits | Settled | §6 | ALG-736, ALG-738, ALG-745 |
| D15 | Leverage and margin modes | Settled | §4, §6 | ALG-736, ALG-740, ALG-756, ALG-760, ALG-762 |
| D16 | Durability, migrations and cutover | The principle is settled; ALG-734, ALG-738 and ALG-768 own the detail | §15 | ALG-734, ALG-738, ALG-754, ALG-758, ALG-768 |
| D17 | Liquidation availability and warnings | Settled | §7 | ALG-741, ALG-744, ALG-746, ALG-763, ALG-764 |
| D18 | Monitoring order and reconnection | Settled | §11 | ALG-744, ALG-748, ALG-764 |
| D19 | Response to a failed stop | Settled, with its bounds | §8 | ALG-746, ALG-748 |
| D20 | Manual changes and ownership | Settled | §10 | ALG-744, ALG-746, ALG-756, ALG-764 |
| D21 | Telegram retries and message semantics | Only the outbox and the PnL line are settled; ALG-749 and ALG-751 own the rest | §1, §12 | ALG-749, ALG-751 |
| D22 | Service lifecycle and deployment | Settled: one owner per store, explicit arming, and a stop that leaves exposure protected | §1, §5 | ALG-752, ALG-754, ALG-767, ALG-768 |
| D23 | Credential storage and redaction | The principle is settled | §15 | ALG-738, ALG-750, ALG-754, ALG-759 |
| D24 | Timing targets | Settled: the PRD sets no timing target, so TradingView's 3 s webhook limit is the only one | §15 | ALG-743, ALG-754, ALG-758 |

## The PRD's example arithmetic

**The p.9 example is consistent with a 150 USDT budget,** 1 % of 15,000 USDT, in
every figure but one:
- 150 / 34.23 floors to 4.38 SOL on a 0.01 step;
- 4.38 SOL is worth 1,025.93 at 234.23;
- its margin at 5× is 205.19;
- p.10's stop-loss example loses 149.93.

**Only the risk line, which says 15.00 USDT, is wrong:** it is off by a factor of
ten.

**Read literally, 15 USDT buys 0.43821 SOL** before the exchange's step. That is
0.43 SOL on Binance and 0.4 on Bybit.

**The messages keep the PRD's fields, but not its numbers.** Tests take their
figures from
[`sizing.py`](../spikes/futures-contract/sizing.py), never from these
illustrative ones.
