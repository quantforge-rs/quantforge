# Futures venues

This page is the living reference behind
[ADR 0002](adr/0002-automation-architecture.md). It records what Binance and
Bybit offer for the v0.3 futures families: which instruments qualify, where each
API lives, what changed recently, and what is still unverified.

ADR 0002 holds the decisions and stays as accepted. This page holds facts that
change, so update it in the same change as the adapter story that finds a
difference.

Facts here were checked on 2026-10-22 against:
- each venue's current documentation and changelog;
- the public endpoints probed by
  [`universe.py`](../spikes/futures-contract/universe.py).

None were checked with signed requests.

## Product universe

The rule is ADR 0002 §3, rule version 1. The universe is computed from venue
metadata each time, never from a fixed list.

| Venue | Family | Kept |
| --- | --- | --- |
| Binance | linear (USDⓈ-M) | `contractType` `PERPETUAL`, `marginAsset` `USDT`, `status` `TRADING` |
| Binance | inverse (COIN-M) | `contractType` `PERPETUAL`, `contractStatus` `TRADING` |
| Bybit | linear | `contractType` `LinearPerpetual`, `settleCoin` `USDT`, `status` `Trading` |
| Bybit | inverse | `contractType` `InversePerpetual`, `status` `Trading` |

Counts from [`universe-2026-10-22.json`](../spikes/futures-contract/universe-2026-10-22.json):

| Venue | Family | Environment | Instruments | Kept | Refused |
| --- | --- | --- | --- | --- | --- |
| Binance | linear | mainnet | 931 | 524 | 407 |
| Binance | inverse | mainnet | 30 | 20 | 10 |
| Binance | linear | demo | 742 | 528 | 214 |
| Binance | inverse | demo | 51 | 20 | 31 |
| Bybit | linear | mainnet | 907 | 799 | 108 |
| Bybit | inverse | mainnet | 26 | 22 | 4 |
| Bybit | linear | testnet | 896 | 787 | 109 |
| Bybit | inverse | testnet | 25 | 21 | 4 |
| Bybit | linear | demo | 907 | 799 | 108 |
| Bybit | inverse | demo | 26 | 22 | 4 |

Every family has trading perpetuals on mainnet and on each venue's test
environment, so no required family is missing.

**What mainnet refuses:**
- **Binance USDⓈ-M:**
  - 225 TradFi perpetuals (`TRADIFI_PERPETUAL`: stocks, commodities, FX);
  - 134 perpetuals in `SETTLING`;
  - 38 USDC-margined, 2 USD1-margined, 2 U-margined and 1 BTC-margined
    perpetual;
  - 4 quarterlies;
  - 1 pending listing.
- **Binance COIN-M:** 10 quarterlies.
- **Bybit linear:** 68 USDC perpetuals and 40 USDT dated futures.
- **Bybit inverse:** 4 dated futures.

**What differs between environments:**
- **Listings differ.** Bybit's testnet lists APE inverse but not FIL or NEAR.
  Binance demo carries stale delivering quarterlies and weekly contracts.
- **Instrument rules differ too.** Binance BTCUSDT steps 0.001 on mainnet but
  0.0001 on demo, and the market-order maximums differ.
- **Read the rules from the environment that is being traded,** never from
  mainnet.
- **Bybit's default listing omits closed and pre-launch instruments,** so its
  refused counts cover only what it returns.

### Units

| Venue | Family | Quantity unit | Face value F | Source of F |
| --- | --- | --- | --- | --- |
| Binance | linear | base asset (BTCUSDT steps 0.001 BTC) | — | — |
| Binance | inverse | contracts | BTC 100 USD; every other coin 10 USD | `contractSize` in `exchangeInfo` |
| Bybit | linear | base coin (BTCUSDT steps 0.001 BTC) | — | — |
| Bybit | inverse | contracts of 1 USD | 1 USD | Bybit's documented rule; no metadata field carries it |

Reference instruments on mainnet:

| Venue | Symbol | Tick | Step | Min qty | Market max | Min notional |
| --- | --- | --- | --- | --- | --- | --- |
| Binance | BTCUSDT | 0.10 | 0.001 | 0.001 | 120 | 50 USDT |
| Binance | SOLUSDT | 0.0100 | 0.01 | 0.01 | 80000 | 5 USDT |
| Binance | BTCUSD_PERP | 0.1 | 1 | 1 | 60000 | none |
| Binance | ETHUSD_PERP | 0.01 | 1 | 1 | 500000 | none |
| Bybit | BTCUSDT | 0.10 | 0.001 | 0.001 | 150.000 | 5 USDT |
| Bybit | SOLUSDT | 0.010 | 0.1 | 0.1 | 12000.0 | 5 USDT |
| Bybit | BTCUSD | 0.10 | 1 | 1 | 5000000 | 5 USD |
| Bybit | SOLUSD | 0.01 | 1 | 1 | 1000000 | 5 USD |

On Binance, use `PRICE_FILTER` and `LOT_SIZE`/`MARKET_LOT_SIZE` for the grid,
never `pricePrecision` or `quantityPrecision`. Bybit adjusts its maximum
quantities twice a month, so they must not be cached indefinitely.

## Routes

| | Binance USDⓈ-M | Binance COIN-M | Bybit (linear and inverse) |
| --- | --- | --- | --- |
| REST, mainnet | `https://fapi.binance.com` | `https://dapi.binance.com` | `https://api.bybit.com` |
| REST, test | demo: `https://demo-fapi.binance.com` (alias `https://testnet.binancefuture.com`) | demo: `https://demo-dapi.binance.com` (alias `https://testnet.binancefuture.com`) | testnet: `https://api-testnet.bybit.com`; demo: `https://api-demo.bybit.com` |
| Public streams, mainnet | `wss://fstream.binance.com/public/…` (depth, book ticker); `/market/…` (mark price, klines, trades) | `wss://dstream.binance.com/ws/<stream>` | `wss://stream.bybit.com/v5/public/{linear,inverse}` |
| Public streams, test | `wss://demo-fstream.binance.com` | `wss://demo-dstream.binance.com` | testnet: `wss://stream-testnet.bybit.com/v5/public/…`; demo: mainnet's public streams |
| Private streams | `wss://fstream.binance.com/private/ws/<listenKey>` | `wss://dstream.binance.com/ws/<listenKey>` | `wss://stream.bybit.com/v5/private`; testnet `stream-testnet`; demo `wss://stream-demo.bybit.com` (private only) |
| Order WebSocket API | `wss://ws-fapi.binance.com/ws-fapi/v1` | `wss://ws-dapi.binance.com/ws-dapi/v1` | `wss://stream.bybit.com/v5/trade`; not on demo |

**Binance's futures testnet is its demo trading.** The `testnet.binancefuture.com`
and `demo-*` hosts return the same data and share one request-weight counter,
and their keys are created on demo.binance.com. QuantForge calls this
environment `demo`.

**Bybit's testnet and demo are separate accounts with separate keys:**
- **Testnet** has its own order book, and liquidity there is thin.
- **Demo** uses mainnet prices.
- A key used on the wrong host is refused with 10003.

**Signed requests go only to the hosts listed for the profile's environment.**
The Spot adapter's `binance.com` suffix check would call `demo-fapi.binance.com`
production, so the futures adapters do not use it.

| Operation | Binance USDⓈ-M | Binance COIN-M | Bybit |
| --- | --- | --- | --- |
| Instruments | `GET /fapi/v1/exchangeInfo` | `GET /dapi/v1/exchangeInfo` | `GET /v5/market/instruments-info` (by category, following `nextPageCursor`) |
| Position mode | `GET/POST /fapi/v1/positionSide/dual` (shared with COIN-M) | `GET/POST /dapi/v1/positionSide/dual` (shared with USDⓈ-M) | `POST /v5/position/switch-mode`; read `positionIdx` from `/v5/position/list` or `/v5/position/symbol-info` |
| Account and margin mode | `GET /fapi/v1/multiAssetsMargin`; per-symbol `marginType` in `GET /fapi/v1/symbolConfig` | `marginType` in `GET /dapi/v1/positionRisk` | `GET /v5/account/info`: `unifiedMarginStatus`, `marginMode` |
| Leverage | `POST /fapi/v1/leverage`; brackets `GET /fapi/v1/leverageBracket` | `POST /dapi/v1/leverage`; brackets `GET /dapi/v2/leverageBracket` | `POST /v5/position/set-leverage` |
| Market entry | `POST /fapi/v1/order` | `POST /dapi/v1/order` | `POST /v5/order/create` (category, `positionIdx` 0) |
| Order query | `GET /fapi/v1/order` (by client id) | `GET /dapi/v1/order` | `GET /v5/order/realtime`, `/v5/order/history` (by `orderLinkId`) |
| Native stop | `POST /fapi/v1/algoOrder`: `CONDITIONAL`, `STOP_MARKET`, `closePosition`, `workingType=MARK_PRICE`, `priceProtect` | `POST /dapi/v1/algoOrder` (partly documented); stops never go to the ordinary order endpoint | `POST /v5/position/trading-stop`: `tpslMode=Full`, `slOrderType=Market`, `slTriggerBy=MarkPrice` |
| Stop read-back | `GET /fapi/v1/algoOrder`, `GET /fapi/v1/openAlgoOrders` | `GET /dapi/v1/openAlgoOrders` | `stopLoss` in `/v5/position/list`; `stopOrderType=StopLoss` in `/v5/order/realtime` |
| Positions and liquidation | `GET /fapi/v3/positionRisk` | `GET /dapi/v1/positionRisk` | `GET /v5/position/list` |
| Balances | `GET /fapi/v3/balance` (`accountAlias`) | `GET /dapi/v1/balance` | `GET /v5/account/wallet-balance` |
| Fills, fees, PnL | `GET /fapi/v1/userTrades` (`realizedPnl`, `commission`); `GET /fapi/v1/income` (`REALIZED_PNL`, `COMMISSION`, `FUNDING_FEE`) | `GET /dapi/v1/userTrades`; `GET /dapi/v1/income` | `GET /v5/execution/list`; `GET /v5/position/closed-pnl`; `GET /v5/account/transaction-log` |
| Private events | `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, `ALGO_UPDATE` | `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE` | topics `position`, `order`, `execution`, `wallet` |
| Account identity | `accountAlias`; spot `GET /api/v3/account` has `uid` | `accountAlias` | `userID` in `GET /v5/user/query-api` |

## Changes the adapters must follow

**Binance:**

| Date | Change |
| --- | --- |
| 2025-12-09 | USDⓈ-M conditional orders (`STOP_MARKET` and the rest) moved to the algo API. `POST /fapi/v1/order` refuses them with −4120 `STOP_ORDER_SWITCH_ALGO`. Algo orders have no margin check until they trigger, so a stop can be `REJECTED` when it fires. An untriggered algo order cannot be modified, only cancelled and placed again. |
| 2025-12-29 | At most 200 open conditional orders across all symbols. |
| 2026-04-23 | USDⓈ-M WebSocket routes split into `/public`, `/market` and `/private`. An unrouted connection receives only public data. |
| 2026-06-30 | USDⓈ-M and COIN-M integrated. They share one position mode: changing it on either host changes both, and it fails with −4067/−4068 while either side has orders or positions. They share one request-weight budget per IP and one order budget per account. Acknowledgements no longer carry `avgPrice`, `cumQuote` or `cumBase`, so fills come from order queries, `userTrades` or the user stream. COIN-M gained `POST /dapi/v1/algoOrder`. |
| 2026-08-26 | `userTrades` covers only the last three months. |
| 2026-10-16 | The income endpoints dropped `page`; page by `startTime` and `endTime`. |

**Bybit:**

| Date | Change |
| --- | --- |
| 2025-02-27 | Market orders take a slippage tolerance, but not together with TP/SL. |
| 2025-06-24 | Closed PnL reports `openFee` and `closeFee`. |
| 2026-06-16 | `/v5/position/symbol-info` reads leverage and `positionIdx` without a position. |
| 2026-08-06 | Inverse perpetuals and futures support hedge mode as well as one-way. |
| 2026-09-01 | A `PendingOpen` instrument status. |

## Accounts and modes

**Binance:**
- **Position mode:** one setting for USDⓈ-M and COIN-M.
- **Multi-assets mode:** USDⓈ-M only; it requires cross margin.
- **Margin type:** set per symbol.
- **Portfolio margin:**
  - PM accounts trade through `papi` only.
  - Classic PM accounts keep using `fapi`/`dapi`, but with a separate user stream.
  - They are detected through `/sapi/v1/account/info` and
    `/sapi/v1/portfolio/account`.

**Bybit:**
- **Account types:**
  - Classic accounts are no longer offered.
  - Unified accounts 1.0 keep inverse contracts in a separate account.
  - Unified accounts 2.0 trade inverse inside the unified account.
  - `unifiedMarginStatus` reports the type: 5 or 6 is 2.0.
- **Margin mode:** account-wide: `ISOLATED_MARGIN`, `REGULAR_MARGIN` (cross) or
  `PORTFOLIO_MARGIN`.
  - Portfolio margin reports no leverage, initial margin or liquidation price.
  - Under cross margin, an inverse position can be collateralised with other
    assets while its PnL settles in the coin.
- **Position mode:** set per symbol or per coin; a symbol setting wins over a
  coin setting, which wins over the default.

## Order identity and retention

**Client order IDs:**

| Venue | Format | Uniqueness |
| --- | --- | --- |
| Binance (`newClientOrderId`, `clientAlgoId`) | `^[.A-Z:/a-z0-9_-]{1,36}$` | Only among open orders, so an ID can be reused once an order ends |
| Bybit (`orderLinkId`) | `[A-Za-z0-9_-]`, at most 36 characters | Always unique; a duplicate is refused with 110072 |

Bybit's position stops carry no client ID.

**Retention on Binance:**
- An order that was cancelled or expired unfilled, including an algo order, can't
  be queried after 3 days.
- No order can be queried after 90 days.
- `userTrades` reaches back 3 months.

**Retention on Bybit:**
- `GET /v5/order/realtime` finds an order by client ID while it is open, and
  after it ends while it is among the account's last 500 closed orders in its
  category. When Bybit restarts the service for an update, those closed orders
  are cleared.
- `GET /v5/order/history` keeps orders that ended unfilled (cancelled, rejected
  or deactivated) for 24 hours and other closed orders for 7 days; after that,
  only orders with fills.
- Executions and closed PnL reach back 2 years, in 7-day windows.
- Demo keeps orders for 7 days.

**Acknowledgements are not fills:**
- Binance's acknowledgements carry no fills since 2026-06-30.
- Bybit's `create-order` returns only the IDs, and says to confirm the order on
  the stream.

### Unknown outcomes

ADR 0002 §9 settles an unknown outcome within two bounds, timed from when the
request was sent and the same on every row below:
- **Propagation delay, 30 seconds:** before it, "not found" proves nothing.
- **Settle window, 60 seconds:** an entry still unsettled then raises an
  incident.

A "not found" also counts only inside the retention windows above.

| Venue | Families | Environments | Query by client ID | "Not found" | Stated visibility delay |
| --- | --- | --- | --- | --- | --- |
| Binance | linear, inverse | mainnet, demo | `GET /fapi/v1/order` or `GET /dapi/v1/order` with `origClientOrderId` | −2013 `NO_SUCH_ORDER` | None. After −1007 or a 503 "Unknown error", the order may have executed, and Binance says to check the user stream or query the order before resending |
| Bybit | linear, inverse | mainnet, testnet, demo | `GET /v5/order/realtime`, then `GET /v5/order/history`, with `orderLinkId` | An empty list from both | None. The history may lag because order handling is asynchronous, and the realtime query may lag in extreme volatility |

No venue states how long an accepted order can stay invisible, so the adapters
measure it on each test environment (unverified item 15).

## Stops and liquidation

**Binance:**
- **Algo stop statuses:** `NEW`, `CANCELED`, `TRIGGERING`, `TRIGGERED`,
  `FINISHED`, `REJECTED` and `EXPIRED`.
  - `EXPIRED` is a system cancel, for example after the position closed.
- **Defaults:** `workingType` is `CONTRACT_PRICE` and `priceProtect` is `false`,
  so ADR 0002 sets both explicitly.
- **Liquidation price:** `liquidationPrice` in `positionRisk`. It is `0` when
  there is none, and `ACCOUNT_UPDATE` does not carry it.
- **Order events:** a liquidation's order has client ID `autoclose-…`; an ADL
  order has `adl_autoclose`.
- **Throttling:** reduce-only and close-position orders are exempt from −1008
  throttling.

**Bybit:**
- **Full-mode position stop:**
  - It closes the whole position, follows its size, and ends with it.
  - It is a single value on the position, changed in place.
  - `/v5/position/list` reports its price but not its trigger basis.
- **Cancels:** an order's `cancelType` tells a user's cancel (`CancelByUser`)
  from a system's.
- **Liquidation:** `liqPrice` is exact under isolated margin and estimated under
  cross. It is empty under portfolio margin or when it falls outside the price
  limits.

## TradingView webhooks

**Requests:**
- **Content type:** `application/json` when the alert message is valid JSON,
  `text/plain` otherwise.
- **Limits:** a request is cancelled after 3 seconds. Only ports 80 and 443,
  IPv4 only, and only for accounts with two-factor authentication.
- **Retries:** an HTTP 5xx response other than 504 is resent after 5 s, up to 3
  times, so a receiver can see 4 deliveries.

**Authentication:**
- TradingView presents a TLS client certificate with subject CN
  `webhook-server@tradingview.com`, which the receiver verifies.
- There is no signature or custom header.
- Requests come from 52.89.214.238, 34.212.75.30, 54.218.53.128 and 52.32.178.7.

**Placeholders:**

| Placeholder | Value |
| --- | --- |
| `{{timenow}}` | The fire time, UTC, to the second |
| `{{time}}` | The bar's open time |
| `{{ticker}}` | The chart symbol, e.g. `BTCUSDT.P` for a Bybit perpetual |
| `{{close}}` and the other prices | Fixed-point numbers |

Messages built with `alert()` do not expand placeholders.

## Unverified items

Each item has a rule for whichever way it resolves: in ADR 0002 where the
outcome changes a decision, otherwise below. The story named here records the
answer, in this table and in its own evidence.

| # | Item | Until verified | Owner |
| --- | --- | --- | --- |
| 1 | COIN-M algo orders: their full lifecycle, `ALGO_UPDATE` on COIN-M, `clientAlgoId`, and when `/dapi/v1/order` starts refusing stops with −4120 | ALG-763 proves the algo API on demo. If it cannot carry the whole lifecycle, the Binance inverse cells block the release until Marko decides. Stops never go to the ordinary order endpoint | ALG-763 |
| 2 | Whether Binance refuses a second `closePosition` algo stop in the same direction (−4130) | A replacement bridges with a reduce-only stop for the current quantity (ADR 0002 §7). If a second `closePosition` stop is accepted, the bridge may be one | ALG-763 |
| 3 | Whether Binance's `accountAlias` is stable and the same for USDⓈ-M and COIN-M; spot `uid` with a futures-only or demo key | Mainnet arming needs a venue account ID. Elsewhere a key fingerprint stands in | ALG-759 |
| 4 | Bybit `GET /v5/user/query-api` on demo | As item 3 | ALG-739 |
| 5 | Reading back the trigger basis of a Bybit position stop | "Protected" needs it read back. If the position stop can't show it, ALG-741 uses a stop whose basis can be read | ALG-741 |
| 6 | Bybit market entry with an attached stop, on testnet and demo | The stop is placed after the fill instead | ALG-741 |
| 7 | Whether removing a Bybit position stop reports `cancelType` | Without it, a removal gets the repair path, and ALG-746 adds the command that leaves a position unprotected on purpose | ALG-741, ALG-746 |
| 8 | Binance `liquidationPrice` of `0` on an open position | Shown as N/A, with a reason | ALG-760 |
| 9 | Whether `userTrades` and `allOrders` return the other family's data since the integration (reports conflict) | Every query filters by symbol | ALG-764 |
| 10 | Inverse trading through the API on Bybit demo | Inverse evidence comes from testnet | ALG-740 |
| 11 | Binance `/private` user stream on demo | ALG-764 records the working route. Without one, the Binance monitoring cells block the release | ALG-764 |
| 12 | The error a portfolio-margin key gets from `fapi` | Portfolio margin is detected up front and refused | ALG-759 |
| 13 | Which Bybit transaction-log type carries inverse funding | Funding comes from whichever type the evidence shows; nothing is counted twice | ALG-764, ALG-747 |
| 14 | Whether `newOrderRespType=RESULT` still returns fills on Binance | Acknowledgements are never read as fills | ALG-762 |
| 15 | How long an accepted order can stay invisible to a query by client ID, on each venue, family and environment | ADR 0002 §9's bounds apply, and the ADR says what changes if a venue exceeds them | ALG-740, ALG-762 |

## Sources

**Binance:**
- [derivatives change log](https://developers.binance.com/docs/derivatives/change-log)
- [CM-UM integration notice](https://developers.binance.com/en/docs/products/derivatives-trading-coin-futures/Important-CM-UM-Integration-Notice)
- [USDⓈ-M WebSocket change notice](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Important-WebSocket-Change-Notice)
- [new algo order](https://developers.binance.com/docs/derivatives/usds-margined-futures/trade/rest-api/New-Algo-Order)
- [USDⓈ-M general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info)
- [USDⓈ-M query order](https://developers.binance.com/docs/derivatives/usds-margined-futures/trade/rest-api/Query-Order)
- [COIN-M query order](https://developers.binance.com/docs/derivatives/coin-margined-futures/trade/rest-api/Query-Order)
- [USDⓈ-M error codes](https://developers.binance.com/docs/derivatives/usds-margined-futures/error-code)

**Bybit:**
- [instruments](https://bybit-exchange.github.io/docs/v5/market/instrument)
- [positions](https://bybit-exchange.github.io/docs/v5/position)
- [trading stop](https://bybit-exchange.github.io/docs/v5/position/trading-stop)
- [create order](https://bybit-exchange.github.io/docs/v5/order/create-order)
- [open orders](https://bybit-exchange.github.io/docs/v5/order/open-order)
- [order history](https://bybit-exchange.github.io/docs/v5/order/order-list)
- [account modes](https://bybit-exchange.github.io/docs/v5/acct-mode)
- [demo trading](https://bybit-exchange.github.io/docs/v5/demo)
- [change log](https://bybit-exchange.github.io/docs/changelog/v5)

**TradingView:**
- [webhook alerts](https://www.tradingview.com/support/solutions/43000529348-how-to-configure-webhook-alerts/)
- [webhook authentication](https://www.tradingview.com/support/solutions/43000680459-webhook-authentication/)
- [resubmission](https://www.tradingview.com/support/solutions/43000735201-webhook-resubmission/)
