-- Schema 7: the tables the development build before job logs (fd75af7) wrote, as it wrote them: schema-6.sql
-- opened by that build, which migrated it, then driven through that build's job store as a server drives it: the
-- running job failed, the queued one run to a success with its three result files, and two more jobs submitted,
-- one left running and one queued. Dumped by Python's sqlite3, with rows a migration must carry forward
-- unchanged. Loaded by tests/storage/migration.rs.
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
CREATE TABLE job_results (
                  hash TEXT PRIMARY KEY,
                  size_bytes INTEGER NOT NULL,
                  data BLOB NOT NULL,
                  stored_at_ms INTEGER NOT NULL
                );
INSERT INTO "job_results" VALUES('sha256:ccebe4813b29b68b0365f83d5007cb249e8ef2b9ff7b908e121c605cfb14433b',125,X'28B52FFD207D2D0300F28514197049DCB0F64A09226C93426A445EFDDF287B58881A4AFFBF100B8079735B271352D6E4CAB1D0A10C254209046FDEA790CC5A27964A7BF561000922267A9B21A5525EC18D5FB92B371F1CF4A4D2EDE5BD2206003EC500E420C29FC1800A5EBDE613',1792454472000);
INSERT INTO "job_results" VALUES('sha256:97f8f018614e0d954b58d186f85d5602af10cf61f3866a30c7bdc620f279fd18',1890,X'28B52FFD606206BD100006564B3B50699C356D88CC7444BDD906219719879F108D4AD206B4D06250E7F420A8DCAB84050C71EEFA4BBDB89A2FF25E8493622B5B63D8CA9DDDDDED4D52350033004100F2AC7A403184A8549068FCC4CA1721861035E4114DFC939FFED3B6BB19999FFA8AEA97D5A49D6D7D0D597B7B6AEEEE3E75D8741D268F6A63B7445A392824E17D35144D78E3188A113FC36E29C3FA774B160AFC0BFE8B1FE327FDC4F9B13FF2E3E7C023D6FF92FC59976D6E53860C809A3493B525548D6580E57401143200476365BD6192B707763680C21841EF033FF532551040598CC41A72ECAE46C17F0D3EA284573F14DFEE5266D461D37598A71EA80B81DFC0D5762D59721C6799B43227BD28A59639A9C559386E4EFA8F0910B08E46AAA040F9864C2701828603904804797159480AC703F98071648084A85149A886444A0A4A950EE08291195A371200F434E494220529A3A095FD9F390C3E9AE94F7B5299C897CBE48E91D344DEEC639EB44D7D36F53E668306FFC8FB5B1DDD813C0CCDEFDA22F4959735DBF915505017D9C9D5F570922DBD7D224F9476E660C398BC8BC099F893406CF813CF7F1313B7351ABA0570D9343E489E5533F373FE21AB38406A354D0F51B6EC76DF21620979D35E4BF496896FE9EB742524EF7141D0DE46C88074BFF28A1FCB869BCF1863AD77BFF2E82B3A3428857F25BD98BF8FB30FE18B9CEED9BB03C2A54F6C22AA0E4F5A5C3A49ED054EED5B23E73DF1894FF7A8',1792454472000);
INSERT INTO "job_results" VALUES('sha256:68023f6c7bc035b19a12cfd343debcdd37b9d4f615b7f00664efff2c669b2b39',1186,X'28B52FFD60A203950D00E6D23F3750AD5A03A4AAEA3A5FC9696CBB9D564565542D35618E27D9F1617EDA586DCED22233E604FA9245775E7DA3F5A0E61824F7D87B4FDFCA142E00320036002D96B8DA8818AB1469448D5AAA1FF3DB5F7AF75E9288BFF2AEF9D329BBB9B3B505EEF65E36DC7BEF5BD0270C3D019869991A4C4758A790782A8D2194A8917A50FEDB9C82B75DF666F6DF7B6104CF31965A2AF1D72E3F47E7C6D16995226D4653AFF9137E84FF809A80079CC4EE525C7705990134AFB907D156A1657C641DE0F48859405C15918C84BD50445F84806F9C0DBC0973270316F40943CF4BD1254B2CFC0ECF3DEDCBB472ACE57CDFF7D5DAA9D6D6CAD0A9B6566BF153FC133FC4FFFDF55AA881ADC2AA2425C9B419C0829999AA0E1220E62149515024294A9D8DFD3B1157336F171D03AC7075FA2A03637DBE7E30F1190565ADF3D8C7F03FF4146BF005084C633A7B8E27C84F467076BFF072BD58C71CCD9006CE02B4C152428390B59EC5455906CB80420CC91D8305AD6D80B385FB97AEFE0BD4EFE7A226CC86D2100175714A555DD026F1812F554D77D1EB578C93BEDAA80CDEF56AE32E8735C8F7BD66A436B54E60677C4A1630D3A98D5103',1792454472000);
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
INSERT INTO "jobs" VALUES(4,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','failed','sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333','{"bundle":"sha256:040711db302c22bf7edb1dccf649ac2c4cb0850ab4a8fa1d4c24f768afad3333","data":{"exchange":"binance","category":"spot","symbol":"币安人生USDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454460000,1792454461000,1792454470000,NULL,'{"code":"internal","message":"the server stopped while the job ran; submit it again"}');
INSERT INTO "jobs" VALUES(5,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','succeeded','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454470000,1792454471000,1792454472000,'[{"name":"metrics","media_type":"application/json","hash":"sha256:ccebe4813b29b68b0365f83d5007cb249e8ef2b9ff7b908e121c605cfb14433b","size_bytes":125},{"name":"trades","media_type":"application/vnd.apache.arrow.file","hash":"sha256:97f8f018614e0d954b58d186f85d5602af10cf61f3866a30c7bdc620f279fd18","size_bytes":1890},{"name":"equity","media_type":"application/vnd.apache.arrow.file","hash":"sha256:68023f6c7bc035b19a12cfd343debcdd37b9d4f615b7f00664efff2c669b2b39","size_bytes":1186}]',NULL);
INSERT INTO "jobs" VALUES(6,'cccccccccccccccccccccccccccccccc','running','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454480000,1792454481000,NULL,NULL,NULL);
INSERT INTO "jobs" VALUES(7,'dddddddddddddddddddddddddddddddd','queued','sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880','{"bundle":"sha256:40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792454520000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}',1792454490000,NULL,NULL,NULL,NULL);
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO "meta" VALUES('schema_version','7');
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
INSERT INTO "schema_migrations" VALUES(7,6,1794437823933,'0.2.0');
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
INSERT INTO "sqlite_sequence" VALUES('jobs',7);
COMMIT;
