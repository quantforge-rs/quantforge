-- Schema 6: the tables the development build before job results (d7e9e36) wrote, as it wrote them: schema-5.sql
-- opened by that build, which migrated it, then two bundles and five jobs written through that build's job store:
-- one succeeded, one failed, one cancelled, one running and one queued. Dumped by Python's sqlite3, with rows a
-- migration must carry forward unchanged. Loaded by tests/storage/migration.rs.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
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
INSERT INTO "bot_runs" VALUES('run-a','binance_spot','BTCUSDT','1m','sma_cross','stopped','{"execution_mode":"DryRun","last_error":null,"last_processed_open_time_ms":1792454460000,"market":{"exchange":"BinanceSpot","interval":"M1","symbol":"BTCUSDT"},"position":{"entry_order_id":null,"entry_price":null,"entry_time_ms":null,"qty":"0"},"run_id":"run-a","started_at_ms":1792454400000,"status":"Stopped","stopped_at_ms":1792454520000,"strategy_config":{"fast":20,"kind":"sma_cross","slow":50},"strategy_name":"sma_cross","updated_at_ms":1792454520000}',1792454400000,1792454520000,1792454520000,NULL,'testnet',NULL,NULL,'adopted',1794144114880,'{"run_id":"run-a","market":{"exchange":"BinanceSpot","symbol":"BTCUSDT","interval":"M1"},"strategy_name":"sma_cross","strategy_config":{"kind":"sma_cross","fast":20,"slow":50},"execution_mode":"DryRun","status":"Stopped","last_processed_open_time_ms":1792454460000,"started_at_ms":1792454400000,"updated_at_ms":1792454520000,"stopped_at_ms":1792454520000,"last_error":null,"position":{"qty":"0","entry_price":null,"entry_time_ms":null,"entry_order_id":null}}');
INSERT INTO "bot_runs" VALUES('run-b','binance_spot','BTCUSDT','1m','sma_cross','running','{"execution_mode":"Live","last_error":null,"last_processed_open_time_ms":1792454460000,"market":{"exchange":"BinanceSpot","interval":"M1","symbol":"BTCUSDT"},"position":{"entry_order_id":"7","entry_price":"3933.07","entry_time_ms":1792454460100,"qty":"0.0254"},"run_id":"run-b","started_at_ms":1792454400000,"status":"Running","stopped_at_ms":null,"strategy_config":{"fast":20,"kind":"sma_cross","slow":50},"strategy_name":"sma_cross","updated_at_ms":1792454520000}',1792454400000,1792454520000,NULL,NULL,NULL,NULL,NULL,'legacy',NULL,'{"run_id":"run-b","market":{"exchange":"BinanceSpot","symbol":"BTCUSDT","interval":"M1"},"strategy_name":"sma_cross","strategy_config":{"kind":"sma_cross","fast":20,"slow":50},"execution_mode":"Live","status":"Running","last_processed_open_time_ms":1792454460000,"started_at_ms":1792454400000,"updated_at_ms":1792454520000,"stopped_at_ms":null,"last_error":null,"position":{"qty":"0.0254","entry_price":"3933.07","entry_time_ms":1792454460100,"entry_order_id":7}}');
CREATE TABLE bundles (
                  code_hash TEXT PRIMARY KEY,
                  size_bytes INTEGER NOT NULL,
                  data BLOB NOT NULL,
                  stored_at_ms INTEGER NOT NULL
                );
INSERT INTO "bundles" VALUES('sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880',256,X'000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F202122232425262728292A2B2C2D2E2F303132333435363738393A3B3C3D3E3F404142434445464748494A4B4C4D4E4F505152535455565758595A5B5C5D5E5F606162636465666768696A6B6C6D6E6F707172737475767778797A7B7C7D7E7F808182838485868788898A8B8C8D8E8F909192939495969798999A9B9C9D9E9FA0A1A2A3A4A5A6A7A8A9AAABACADAEAFB0B1B2B3B4B5B6B7B8B9BABBBCBDBEBFC0C1C2C3C4C5C6C7C8C9CACBCCCDCECFD0D1D2D3D4D5D6D7D8D9DADBDCDDDEDFE0E1E2E3E4E5E6E7E8E9EAEBECEDEEEFF0F1F2F3F4F5F6F7F8F9FAFBFCFDFEFF',1792454410000);
INSERT INTO "bundles" VALUES('sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333',32,X'7175616E74666F7267652E746F6D6C0000000000000000000000000000000078',1792454420000);
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
INSERT INTO "candles" VALUES('binance_spot','BTCUSDT','1m',1792454400000,1792454459999,'67012.34000000','67020.00000000','67001.10000000','67015.50000000','12.04510000',1234);
INSERT INTO "candles" VALUES('binance_spot','BTCUSDT','1m',1792540800000,1792540859999,'67100.00','67100.00','67100.00','67100.00','1.000',7);
INSERT INTO "candles" VALUES('binance_spot','BTCUSDT','1m',1792540860000,1792540919999,'67100.00','67105.50','67099.90','67101.10','0.50000000',NULL);
INSERT INTO "candles" VALUES('binance_spot','ETHUSDT','8h',1792454400000,1792483199999,'3500.10','3510.00','3490.00','3505.55','100.0000',55);
CREATE TABLE "closed_trades" (
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
INSERT INTO "closed_trades" VALUES(1,'run-a','BTCUSDT',1792454400000,1792454520000,'67012.34','67015.50','0.010','0.0316','7','8');
INSERT INTO "closed_trades" VALUES(2,'run-a','BTCUSDT',1792454400000,1792454520000,'67012.34','67015.50','0.010','0.0316',NULL,NULL);
CREATE TABLE jobs (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  job_id TEXT NOT NULL UNIQUE,
                  state TEXT NOT NULL,
                  bundle_hash TEXT NOT NULL,
                  request_json TEXT NOT NULL,
                  submitted_at_ms INTEGER NOT NULL,
                  started_at_ms INTEGER,
                  finished_at_ms INTEGER,
                  results_json TEXT,
                  failure_json TEXT
                );
INSERT INTO "jobs" VALUES(1,'00000000000000000000000000000001','succeeded','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454430000,1792454431000,1792454432000,'[{"name":"report","media_type":"application/json","hash":"sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","size_bytes":2}]',NULL);
INSERT INTO "jobs" VALUES(2,'ffffffffffffffffffffffffffffffff','failed','sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333','{"bundle":"sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454440000,1792454441000,1792454441500,NULL,'{"code":"unavailable","message":"this server cannot run Python bundles yet; a later QuantForge release brings the worker runtime"}');
INSERT INTO "jobs" VALUES(3,'0123456789abcdef0123456789abcdef','cancelled','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454450000,NULL,1792454452000,NULL,NULL);
INSERT INTO "jobs" VALUES(4,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','running','sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333','{"bundle":"sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333","data":{"exchange":"binance","category":"spot","symbol":"币安人生USDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454460000,1792454461000,NULL,NULL,NULL);
INSERT INTO "jobs" VALUES(5,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','queued','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454470000,NULL,NULL,NULL,NULL);
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO "meta" VALUES('schema_version','6');
CREATE TABLE "order_events" (
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
INSERT INTO "order_events" VALUES(1,'run-b','BTCUSDT','BUY','MARKET','FILLED','7','qf-entry-1',1792454460100,'{"avg_price":null,"client_order_id":"qf-entry-1","cumulative_quote_qty":"99.90","executed_qty":"0.0254","fills":[{"commission":"0.0000254","commission_asset":"BTC","price":"3933.07","qty":"0.0254","trade_id":"42"}],"order_id":"7","order_type":"MARKET","raw":{"orderId":7,"status":"FILLED","workingTime":1792454460100},"requested_qty":null,"requested_quote_qty":"100","side":"Buy","status":"Filled","symbol":"BTCUSDT","transact_time_ms":1792454460100}','{"symbol":"BTCUSDT","side":"Buy","order_type":"MARKET","status":"Filled","order_id":7,"client_order_id":"qf-entry-1","requested_qty":null,"requested_quote_qty":"100","executed_qty":"0.0254","cumulative_quote_qty":"99.90","avg_price":null,"transact_time_ms":1792454460100,"fills":[{"price":"3933.07","qty":"0.0254","commission":"0.0000254","commission_asset":"BTC","trade_id":42}],"raw":{"orderId":7,"status":"FILLED","workingTime":1792454460100}}');
INSERT INTO "order_events" VALUES(2,'run-a','BTCUSDT','SELL','MARKET','FILLED',NULL,'qf-dry-1',1792454520000,'{"symbol":"BTCUSDT","side":"Sell","order_type":"MARKET","status":"Filled","order_id":null,"client_order_id":"qf-dry-1","requested_qty":"0.010","requested_quote_qty":null,"executed_qty":"0.010","cumulative_quote_qty":"670.155","avg_price":"67015.50","transact_time_ms":1792454520000,"fills":[],"raw":{"execution_mode":"dry_run"}}','{"symbol":"BTCUSDT","side":"Sell","order_type":"MARKET","status":"Filled","order_id":null,"client_order_id":"qf-dry-1","requested_qty":"0.010","requested_quote_qty":null,"executed_qty":"0.010","cumulative_quote_qty":"670.155","avg_price":"67015.50","transact_time_ms":1792454520000,"fills":[],"raw":{"execution_mode":"dry_run"}}');
CREATE TABLE schema_migrations (
                  version INTEGER PRIMARY KEY,
                  from_version INTEGER NOT NULL,
                  applied_at_ms INTEGER NOT NULL,
                  quantforge_version TEXT NOT NULL
                );
INSERT INTO "schema_migrations" VALUES(5,4,1794144114814,'0.2.0');
INSERT INTO "schema_migrations" VALUES(6,5,1794425649360,'0.2.0');
CREATE TABLE segment_dirty (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  UNIQUE (exchange, symbol, interval, window_start_ms)
);
INSERT INTO "segment_dirty" VALUES(1,'binance_spot','BTCUSDT','1m',1792540800000);
INSERT INTO "segment_dirty" VALUES(2,'binance_spot','ETHUSDT','8h',1792454400000);
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
INSERT INTO "segment_index" VALUES('binance_spot','BTCUSDT','1m',1792454400000,1792454400000,1792454400000,1,'sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a');
CREATE TABLE segments (
  hash TEXT PRIMARY KEY,
  format INTEGER NOT NULL,
  payload_len INTEGER NOT NULL,
  data BLOB NOT NULL
);
INSERT INTO "segments" VALUES('sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a',1,113,X'28B52FFD20719D0200C40351465345474D4E540100000000989B56A10100005F829CD20400010E36373031322E33343032302E30312E3131352E350B31322E30343531303030300900201360081070899119009CC7B5B122FEC02501');
CREATE TABLE store_control (
                  id INTEGER PRIMARY KEY CHECK (id = 1),
                  store_id TEXT NOT NULL,
                  environment TEXT,
                  activation TEXT NOT NULL,
                  activation_epoch INTEGER NOT NULL,
                  changed_at_ms INTEGER NOT NULL
                );
INSERT INTO "store_control" VALUES(1,'fff1773bbe5d4089b0e5eb358278aeb3','testnet','inactive',0,1794144114857);
CREATE INDEX idx_candles_market_time
  ON candles(exchange, symbol, interval, open_time_ms);
CREATE INDEX idx_bot_runs_market
  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);
CREATE INDEX idx_order_events_run
                  ON order_events(run_id, seq DESC);
CREATE INDEX idx_closed_trades_run
                  ON closed_trades(run_id, seq DESC);
CREATE INDEX idx_jobs_state
                  ON jobs(state, seq);
DELETE FROM "sqlite_sequence";
INSERT INTO "sqlite_sequence" VALUES('segment_dirty',2);
INSERT INTO "sqlite_sequence" VALUES('order_events',2);
INSERT INTO "sqlite_sequence" VALUES('closed_trades',2);
INSERT INTO "sqlite_sequence" VALUES('jobs',5);
COMMIT;
