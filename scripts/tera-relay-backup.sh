#!/bin/sh
set -e

SERVER_IP="155.103.80.244"
SERVER_PORT="19003"
RELAY_BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/tera-relay"

usage() {
	echo "Usage:"
	echo "  $0 start --socks5 <host:port>     egress via un proxy SOCKS5 (ex: SSH -D sur une box whiteliste)"
	echo "  $0 start --to <host:port>         egress via un forwarder distant (ex: socat sur OVH whiteliste)"
	echo "  $0 stop                           arrete le relais et retire l'alias"
	echo "  $0 test <host:port|--socks5 h:p>  teste la capture loopback (bannière du serveur cible)"
	exit 1
}

require_bin() {
	[ -x "$RELAY_BIN" ] || { echo "binaire absent: $RELAY_BIN (cargo build -p tera-relay --release)"; exit 1; }
}

case "$1" in
start)
	require_bin
	shift
	MODE="$1"; ARG="$2"
	[ -n "$ARG" ] || usage
	sudo ifconfig lo0 alias "$SERVER_IP" 2>/dev/null || true
	if [ "$MODE" = "--socks5" ]; then
		exec "$RELAY_BIN" --listen "$SERVER_IP:$SERVER_PORT" --target "$SERVER_IP:$SERVER_PORT" --socks5 "$ARG"
	elif [ "$MODE" = "--to" ]; then
		exec "$RELAY_BIN" --listen "$SERVER_IP:$SERVER_PORT" --target "$ARG"
	else
		usage
	fi
	;;
stop)
	pkill -f "tera-relay --listen $SERVER_IP:$SERVER_PORT" 2>/dev/null || true
	sudo ifconfig lo0 -alias "$SERVER_IP" 2>/dev/null || true
	echo "relais arrete, alias retire"
	;;
test)
	require_bin
	shift
	sudo ifconfig lo0 alias "$SERVER_IP" 2>/dev/null || true
	if [ "$1" = "--socks5" ]; then
		"$RELAY_BIN" --listen "$SERVER_IP:$SERVER_PORT" --target github.com:22 --socks5 "$2" &
	else
		TARGET="${1:-github.com:22}"
		"$RELAY_BIN" --listen "$SERVER_IP:$SERVER_PORT" --target "$TARGET" &
	fi
	R=$!
	sleep 1
	python3 -c 'import socket;s=socket.socket();s.settimeout(6)
try:
 s.connect(("155.103.80.244",19003)); print("capture OK, recu:",s.recv(64))
except Exception as e: print("echec:",repr(e))
finally: s.close()'
	kill "$R" 2>/dev/null || true
	wait "$R" 2>/dev/null || true
	sudo ifconfig lo0 -alias "$SERVER_IP" 2>/dev/null || true
	;;
*)
	usage
	;;
esac
