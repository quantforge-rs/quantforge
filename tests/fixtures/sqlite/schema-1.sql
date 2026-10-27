-- Schema 1: the tables QuantForge 0.1.0 wrote, as that build wrote them, with rows a
-- migration must carry forward unchanged. Loaded by tests/storage/migration.rs.
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS candles (
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

CREATE INDEX IF NOT EXISTS idx_candles_market_time
  ON candles(exchange, symbol, interval, open_time_ms);

INSERT INTO meta VALUES ('schema_version', '1');
INSERT INTO candles VALUES ('binance_spot', 'BTCUSDT', '1m', 1792454400000, 1792454459999, '67012.34000000', '67020.00000000', '67001.10000000', '67015.50000000', '12.04510000', 1234);
INSERT INTO candles VALUES ('binance_spot', 'BTCUSDT', '1m', 1792540800000, 1792540859999, '67100.00', '67100.00', '67100.00', '67100.00', '1.000', 7);
INSERT INTO candles VALUES ('binance_spot', 'BTCUSDT', '1m', 1792540860000, 1792540919999, '67100.00', '67105.50', '67099.90', '67101.10', '0.50000000', NULL);
INSERT INTO candles VALUES ('binance_spot', 'ETHUSDT', '8h', 1792454400000, 1792483199999, '3500.10', '3510.00', '3490.00', '3505.55', '100.0000', 55);
