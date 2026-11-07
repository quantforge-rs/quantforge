#!/usr/bin/env bash
# The PostgreSQL servers the tests run against (docs/ci.md): one that also
# serves TLS to `localhost`, under a CA made for the run, and a weak one,
# with fsync off, whose refusal the durability tests show. CI starts both
# as service containers and runs `configure`; `start` runs the same two
# locally with docker and prints the variables the tests read.
#
#   eval "$(scripts/postgres-test-servers.sh start 16)"
#   cargo test --all-features --locked
#   scripts/postgres-test-servers.sh leftovers
#   scripts/postgres-test-servers.sh stop
#
#   scripts/postgres-test-servers.sh configure <server> <weak> <tls directory>
#   scripts/postgres-test-servers.sh leftovers <container>...
#
# Keep it bash 3.2 safe (macOS /bin/bash), like file-size-check.sh.
set -euo pipefail

user=quant
password=test-pw
server_name=quantforge-test-postgres
weak_name=quantforge-test-postgres-weak
server_port=54329
weak_port=54330

die() {
  echo "postgres-test-servers: $*" >&2
  exit 1
}

usage() {
  sed -n '9,15p' "$0" >&2
  exit 2
}

# psql inside a container, over its socket, as the user the tests use
sql() {
  local container=$1
  shift
  docker exec "$container" psql -X -q -At -v ON_ERROR_STOP=1 -U "$user" -d postgres "$@"
}

# waits until the container's server answers on TCP: during its first start
# the image runs a server on the socket alone
wait_ready() {
  local container=$1 tries=0
  until docker exec "$container" pg_isready -q -h 127.0.0.1 -U "$user"; do
    tries=$((tries + 1))
    [ "$tries" -lt 60 ] || die "$container never answered on TCP"
    sleep 1
  done
}

# waits until a setting reads what a reload gave it
wait_setting() {
  local container=$1 name=$2 expected=$3 tries=0
  until [ "$(sql "$container" -c "SHOW $name" 2>/dev/null || true)" = "$expected" ]; do
    tries=$((tries + 1))
    if [ "$tries" -ge 50 ]; then
      docker logs --tail 20 "$container" >&2 || true
      die "$name never became $expected on $container"
    fi
    sleep 0.2
  done
}

# a CA and a certificate it signs for localhost, in $1
make_certificates() {
  local dir=$1
  mkdir -p "$dir"
  cat > "$dir/openssl.cnf" <<'EOF'
[req]
distinguished_name = name
[name]
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[server]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost
EOF
  {
    openssl req -x509 -new -nodes -newkey rsa:2048 -days 7 -subj "/CN=QuantForge test CA" \
      -config "$dir/openssl.cnf" -extensions ca -keyout "$dir/ca.key" -out "$dir/ca.crt" &&
      openssl req -new -nodes -newkey rsa:2048 -subj "/CN=localhost" \
        -config "$dir/openssl.cnf" -keyout "$dir/server.key" -out "$dir/server.csr" &&
      openssl x509 -req -days 7 -in "$dir/server.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
        -CAcreateserial -extfile "$dir/openssl.cnf" -extensions server -out "$dir/server.crt"
  } > "$dir/openssl.log" 2>&1 || {
    cat "$dir/openssl.log" >&2
    die "openssl could not make the certificates"
  }
}

# TLS on the server in $1, with the certificate in $2
serve_tls() {
  local container=$1 dir=$2 home=/var/lib/postgresql
  docker cp "$dir/server.crt" "$container:$home/server.crt"
  docker cp "$dir/server.key" "$container:$home/server.key"
  docker exec -u root "$container" chown postgres:postgres "$home/server.crt" "$home/server.key"
  docker exec -u root "$container" chmod 600 "$home/server.key"
  sql "$container" \
    -c "ALTER SYSTEM SET ssl_cert_file = '$home/server.crt'" \
    -c "ALTER SYSTEM SET ssl_key_file = '$home/server.key'" \
    -c "ALTER SYSTEM SET ssl = on" \
    -c "SELECT pg_reload_conf()" > /dev/null
  wait_setting "$container" ssl on
}

# fsync off on the server in $1, in its configuration file: the durability
# test turns it on with ALTER SYSTEM, then resets it to what the file says
weaken() {
  local container=$1 file
  file=$(sql "$container" -c "SHOW config_file")
  docker exec "$container" sh -c "echo 'fsync = off' >> '$file'"
  sql "$container" -c "SELECT pg_reload_conf()" > /dev/null
  wait_setting "$container" fsync off
}

configure() {
  local server=$1 weak=$2 dir=$3
  wait_ready "$server"
  wait_ready "$weak"
  make_certificates "$dir"
  serve_tls "$server" "$dir"
  weaken "$weak"
  echo "server: $(sql "$server" -c 'SHOW server_version'), TLS on" >&2
  echo "weak server: $(sql "$weak" -c 'SHOW server_version'), fsync off" >&2
}

# fails, naming them, on any database or role a test left on the servers
leftovers() {
  local container left status=0
  for container in "$@"; do
    left=$(sql "$container" -c "
      SELECT 'database ' || datname FROM pg_database
      WHERE datname NOT IN ('postgres', 'template0', 'template1')
      UNION ALL
      SELECT 'role ' || rolname FROM pg_roles
      WHERE rolname <> current_user AND rolname NOT LIKE 'pg\_%'
      ORDER BY 1")
    if [ -n "$left" ]; then
      echo "$container still holds what a test made:" >&2
      echo "$left" >&2
      status=1
    fi
  done
  [ "$status" -eq 0 ] && echo "nothing left behind on $*" >&2
  return "$status"
}

start() {
  local major=$1 dir=$2 name port
  docker rm -f "$server_name" "$weak_name" > /dev/null 2>&1 || true
  for name in "$server_name" "$weak_name"; do
    port=$server_port
    [ "$name" = "$weak_name" ] && port=$weak_port
    docker run -d --name "$name" -e POSTGRES_USER="$user" -e POSTGRES_PASSWORD="$password" \
      -e POSTGRES_DB=postgres -p "127.0.0.1:$port:5432" "postgres:$major" > /dev/null
  done
  configure "$server_name" "$weak_name" "$dir"
  echo "export QUANTFORGE_TEST_POSTGRES_REQUIRED=1"
  echo "export QUANTFORGE_TEST_POSTGRES_URL='postgres://$user:$password@127.0.0.1:$server_port/postgres?sslmode=disable'"
  echo "export QUANTFORGE_TEST_POSTGRES_TLS_URL='postgres://$user:$password@localhost:$server_port/postgres?sslmode=verify-full&sslrootcert=$dir/ca.crt'"
  echo "export QUANTFORGE_TEST_POSTGRES_WEAK_URL='postgres://$user:$password@127.0.0.1:$weak_port/postgres?sslmode=disable'"
}

command=${1:-}
case "$command" in
  start)
    [ $# -eq 2 ] || usage
    temporary=${TMPDIR:-/tmp}
    start "$2" "${temporary%/}/quantforge-test-postgres"
    ;;
  configure)
    [ $# -eq 4 ] || usage
    configure "$2" "$3" "$4"
    ;;
  leftovers)
    shift
    if [ $# -eq 0 ]; then
      set -- "$server_name" "$weak_name"
    fi
    leftovers "$@"
    ;;
  stop)
    [ $# -eq 1 ] || usage
    docker rm -f "$server_name" "$weak_name" > /dev/null 2>&1 || true
    ;;
  *)
    usage
    ;;
esac
