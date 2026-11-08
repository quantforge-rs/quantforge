-- Schema 5: the tables the development build before job storage (122ec4c) wrote, as it wrote them: schema-4.sql
-- migrated by that build, bound to testnet and run-a adopted through its CLI, then dumped by sqlite3, with rows a
-- migration must carry forward unchanged. Loaded by tests/storage/migration.rs.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO meta VALUES('schema_version','5');
CREATE TABLE candles (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  open_time_ms INTEGER NOT NULL,
  close_time_ms INTEGER NOT NULL,
  open TEXT NOT NULL,
  high TEXT NOT NULL,
  low TEXT NOT NULL,
  close TEXT NOT NULL,
  volume TEXT NOT NULL,
  trades INTEGER,
  PRIMARY KEY (exchange, symbol, interval, open_time_ms)
);
INSERT INTO candles VALUES('binance_spot','BTCUSDT','1m',1792454400000,1792454459999,'67012.34000000','67020.00000000','67001.10000000','67015.50000000','12.04510000',1234);
INSERT INTO candles VALUES('binance_spot','BTCUSDT','1m',1792540800000,1792540859999,'67100.00','67100.00','67100.00','67100.00','1.000',7);
INSERT INTO candles VALUES('binance_spot','BTCUSDT','1m',1792540860000,1792540919999,'67100.00','67105.50','67099.90','67101.10','0.50000000',NULL);
INSERT INTO candles VALUES('binance_spot','ETHUSDT','8h',1792454400000,1792483199999,'3500.10','3510.00','3490.00','3505.55','100.0000',55);
CREATE TABLE bot_runs (
  run_id TEXT PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  strategy_name TEXT NOT NULL,
  status TEXT NOT NULL,
  state_json TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  stopped_at_ms INTEGER,
  last_error TEXT
, environment TEXT, account_kind TEXT, account_id TEXT, identity_source TEXT NOT NULL DEFAULT 'legacy', identity_at_ms INTEGER, legacy_state_json TEXT);
INSERT INTO bot_runs VALUES('run-a','binance_spot','BTCUSDT','1m','sma_cross','stopped','{"execution_mode":"DryRun","last_error":null,"last_processed_open_time_ms":1792454460000,"market":{"exchange":"BinanceSpot","interval":"M1","symbol":"BTCUSDT"},"position":{"entry_order_id":null,"entry_price":null,"entry_time_ms":null,"qty":"0"},"run_id":"run-a","started_at_ms":1792454400000,"status":"Stopped","stopped_at_ms":1792454520000,"strategy_config":{"fast":20,"kind":"sma_cross","slow":50},"strategy_name":"sma_cross","updated_at_ms":1792454520000}',1792454400000,1792454520000,1792454520000,NULL,'testnet',NULL,NULL,'adopted',1794144114880,'{"run_id":"run-a","market":{"exchange":"BinanceSpot","symbol":"BTCUSDT","interval":"M1"},"strategy_name":"sma_cross","strategy_config":{"kind":"sma_cross","fast":20,"slow":50},"execution_mode":"DryRun","status":"Stopped","last_processed_open_time_ms":1792454460000,"started_at_ms":1792454400000,"updated_at_ms":1792454520000,"stopped_at_ms":1792454520000,"last_error":null,"position":{"qty":"0","entry_price":null,"entry_time_ms":null,"entry_order_id":null}}');
INSERT INTO bot_runs VALUES('run-b','binance_spot','BTCUSDT','1m','sma_cross','running','{"execution_mode":"Live","last_error":null,"last_processed_open_time_ms":1792454460000,"market":{"exchange":"BinanceSpot","interval":"M1","symbol":"BTCUSDT"},"position":{"entry_order_id":"7","entry_price":"3933.07","entry_time_ms":1792454460100,"qty":"0.0254"},"run_id":"run-b","started_at_ms":1792454400000,"status":"Running","stopped_at_ms":null,"strategy_config":{"fast":20,"kind":"sma_cross","slow":50},"strategy_name":"sma_cross","updated_at_ms":1792454520000}',1792454400000,1792454520000,NULL,NULL,NULL,NULL,NULL,'legacy',NULL,'{"run_id":"run-b","market":{"exchange":"BinanceSpot","symbol":"BTCUSDT","interval":"M1"},"strategy_name":"sma_cross","strategy_config":{"kind":"sma_cross","fast":20,"slow":50},"execution_mode":"Live","status":"Running","last_processed_open_time_ms":1792454460000,"started_at_ms":1792454400000,"updated_at_ms":1792454520000,"stopped_at_ms":null,"last_error":null,"position":{"qty":"0.0254","entry_price":"3933.07","entry_time_ms":1792454460100,"entry_order_id":7}}');
CREATE TABLE segments (
  hash TEXT PRIMARY KEY,
  format INTEGER NOT NULL,
  payload_len INTEGER NOT NULL,
  data BLOB NOT NULL
);
INSERT INTO segments VALUES('sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a',1,113,X'28b52ffd20719d0200c40351465345474d4e540100000000989b56a10100005f829cd20400010e36373031322e33343032302e30312e3131352e350b31322e30343531303030300900201360081070899119009cc7b5b122fec02501');
CREATE TABLE segment_index (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  first_open_ms INTEGER NOT NULL,
  last_open_ms INTEGER NOT NULL,
  bar_count INTEGER NOT NULL,
  hash TEXT NOT NULL,
  PRIMARY KEY (exchange, symbol, interval, window_start_ms)
);
INSERT INTO segment_index VALUES('binance_spot','BTCUSDT','1m',1792454400000,1792454400000,1792454400000,1,'sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a');
CREATE TABLE segment_dirty (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  UNIQUE (exchange, symbol, interval, window_start_ms)
);
INSERT INTO segment_dirty VALUES(1,'binance_spot','BTCUSDT','1m',1792540800000);
INSERT INTO segment_dirty VALUES(2,'binance_spot','ETHUSDT','8h',1792454400000);
CREATE TABLE IF NOT EXISTS "order_events" (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  side TEXT NOT NULL,
                  order_type TEXT NOT NULL,
                  status TEXT NOT NULL,
                  order_id TEXT,
                  client_order_id TEXT,
                  transact_time_ms INTEGER,
                  raw_json TEXT NOT NULL,
                  legacy_raw_json TEXT
                );
INSERT INTO order_events VALUES(1,'run-b','BTCUSDT','BUY','MARKET','FILLED','7','qf-entry-1',1792454460100,'{"avg_price":null,"client_order_id":"qf-entry-1","cumulative_quote_qty":"99.90","executed_qty":"0.0254","fills":[{"commission":"0.0000254","commission_asset":"BTC","price":"3933.07","qty":"0.0254","trade_id":"42"}],"order_id":"7","order_type":"MARKET","raw":{"orderId":7,"status":"FILLED","workingTime":1792454460100},"requested_qty":null,"requested_quote_qty":"100","side":"Buy","status":"Filled","symbol":"BTCUSDT","transact_time_ms":1792454460100}','{"symbol":"BTCUSDT","side":"Buy","order_type":"MARKET","status":"Filled","order_id":7,"client_order_id":"qf-entry-1","requested_qty":null,"requested_quote_qty":"100","executed_qty":"0.0254","cumulative_quote_qty":"99.90","avg_price":null,"transact_time_ms":1792454460100,"fills":[{"price":"3933.07","qty":"0.0254","commission":"0.0000254","commission_asset":"BTC","trade_id":42}],"raw":{"orderId":7,"status":"FILLED","workingTime":1792454460100}}');
INSERT INTO order_events VALUES(2,'run-a','BTCUSDT','SELL','MARKET','FILLED',NULL,'qf-dry-1',1792454520000,'{"symbol":"BTCUSDT","side":"Sell","order_type":"MARKET","status":"Filled","order_id":null,"client_order_id":"qf-dry-1","requested_qty":"0.010","requested_quote_qty":null,"executed_qty":"0.010","cumulative_quote_qty":"670.155","avg_price":"67015.50","transact_time_ms":1792454520000,"fills":[],"raw":{"execution_mode":"dry_run"}}','{"symbol":"BTCUSDT","side":"Sell","order_type":"MARKET","status":"Filled","order_id":null,"client_order_id":"qf-dry-1","requested_qty":"0.010","requested_quote_qty":null,"executed_qty":"0.010","cumulative_quote_qty":"670.155","avg_price":"67015.50","transact_time_ms":1792454520000,"fills":[],"raw":{"execution_mode":"dry_run"}}');
CREATE TABLE IF NOT EXISTS "closed_trades" (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  run_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  entry_time_ms INTEGER NOT NULL,
                  exit_time_ms INTEGER NOT NULL,
                  entry_price TEXT NOT NULL,
                  exit_price TEXT NOT NULL,
                  qty TEXT NOT NULL,
                  gross_quote_pnl TEXT NOT NULL,
                  entry_order_id TEXT,
                  exit_order_id TEXT
                );
INSERT INTO closed_trades VALUES(1,'run-a','BTCUSDT',1792454400000,1792454520000,'67012.34','67015.50','0.010','0.0316','7','8');
INSERT INTO closed_trades VALUES(2,'run-a','BTCUSDT',1792454400000,1792454520000,'67012.34','67015.50','0.010','0.0316',NULL,NULL);
CREATE TABLE store_control (
                  id INTEGER PRIMARY KEY CHECK (id = 1),
                  store_id TEXT NOT NULL,
                  environment TEXT,
                  activation TEXT NOT NULL,
                  activation_epoch INTEGER NOT NULL,
                  changed_at_ms INTEGER NOT NULL
                );
INSERT INTO store_control VALUES(1,'fff1773bbe5d4089b0e5eb358278aeb3','testnet','inactive',0,1794144114857);
CREATE TABLE schema_migrations (
                  version INTEGER PRIMARY KEY,
                  from_version INTEGER NOT NULL,
                  applied_at_ms INTEGER NOT NULL,
                  quantforge_version TEXT NOT NULL
                );
INSERT INTO schema_migrations VALUES(5,4,1794144114814,'0.2.0');
INSERT INTO sqlite_sequence VALUES('segment_dirty',2);
INSERT INTO sqlite_sequence VALUES('order_events',2);
INSERT INTO sqlite_sequence VALUES('closed_trades',2);
CREATE INDEX idx_candles_market_time
  ON candles(exchange, symbol, interval, open_time_ms);
CREATE INDEX idx_bot_runs_market
  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);
CREATE INDEX idx_order_events_run
                  ON order_events(run_id, seq DESC);
CREATE INDEX idx_closed_trades_run
                  ON closed_trades(run_id, seq DESC);
COMMIT;
