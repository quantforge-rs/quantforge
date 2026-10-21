# PostgreSQL bridge spike

The evidence for [ADR 0001](../../docs/adr/0001-postgres-client-strategy.md):
how QuantForge's synchronous storage ports reach PostgreSQL through
tokio-postgres from every context that calls them. A standalone Cargo
project, outside the published crate and outside CI; ALG-404 builds the
real adapter.

- `src/runner.rs`: the bridge's own runtime, and how synchronous code waits
  on it.
- `src/bridge.rs`: the deadpool pool on that runtime, with deadlines,
  cancellation, and error kinds.
- `src/tls.rs`: rustls with `ring`, and libpq's `sslmode`s mapped onto
  tokio-postgres.
- `src/store.rs`: `quantforge::CandleStore` over the bridge.
- `tests/`: one file per context or property. `control.rs` and `runner.rs`
  need no database, and neither does `tls.rs`; the rest skip without one.

## Running

You need PostgreSQL's binaries and, for the Python spike, a Python 3.9 or
later with its development files. A throwaway cluster on a free port, with
TLS from a CA made for it:

```bash
PG=$(mktemp -d)/pg
initdb -D "$PG" -U spike --auth=trust -E UTF8 --locale=C
cd "$PG"
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=spike CA" \
  -keyout ca.key -out ca.crt
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
  -keyout server.key -out server.csr
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\n' > server.ext
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -days 30 -extfile server.ext -out server.crt
chmod 600 server.key
cat >> postgresql.conf <<CONF
port = 55432
listen_addresses = '127.0.0.1'
unix_socket_directories = ''
ssl = on
ssl_cert_file = 'server.crt'
ssl_key_file = 'server.key'
max_connections = 60
CONF
pg_ctl -D "$PG" -l "$PG/server.log" -w start
createdb -h 127.0.0.1 -p 55432 -U spike spike
cd -
```

`unix_socket_directories = ''` serves TCP only, since a socket under a long
temporary path exceeds the 103 bytes a socket path may have.

Then, from this directory:

```bash
export SPIKE_DATABASE_URL='postgres://spike@127.0.0.1:55432/spike?sslmode=disable'
export SPIKE_TLS_DATABASE_URL="postgres://spike@localhost:55432/spike?sslmode=verify-full&sslrootcert=$PG/ca.crt"
export PYO3_PYTHON=$(command -v python3)
cargo test --features python -- --test-threads=1
```

One thread at a time, because the server spike measures timing. Stop and
remove the cluster afterwards:

```bash
pg_ctl -D "$PG" stop && rm -rf "$PG"
```
