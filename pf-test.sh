#!/bin/sh
set -u
FAKE_IP=198.51.100.9
FAKE_PORT=7800
SRV_PORT=8877
EXCLUDE_SPORT=55000
tmp="${TMPDIR:-/tmp}"

cat > "$tmp/tera_tc.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/select.h>
#include <netinet/in.h>
#include <arpa/inet.h>
int main(int argc, char **argv) {
    struct sockaddr_in target;
    memset(&target, 0, sizeof target);
    target.sin_family = AF_INET;
    target.sin_port = htons((unsigned short)atoi(argv[2]));
    inet_pton(AF_INET, argv[1], &target.sin_addr);
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &one, sizeof one);
    if (argc > 3) {
        struct sockaddr_in src;
        memset(&src, 0, sizeof src);
        src.sin_family = AF_INET;
        src.sin_port = htons((unsigned short)atoi(argv[3]));
        src.sin_addr.s_addr = INADDR_ANY;
        if (bind(fd, (struct sockaddr *)&src, sizeof src) != 0) { perror("bind"); return 2; }
    }
    fcntl(fd, F_SETFL, O_NONBLOCK);
    connect(fd, (struct sockaddr *)&target, sizeof target);
    fd_set writable;
    FD_ZERO(&writable);
    FD_SET(fd, &writable);
    struct timeval timeout = { 3, 0 };
    if (select(fd + 1, NULL, &writable, NULL, &timeout) <= 0) { printf("TIMEOUT (non redirige)\n"); return 1; }
    int error = 0;
    socklen_t len = sizeof error;
    getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &len);
    if (error) { printf("connect refuse (non redirige)\n"); return 1; }
    fcntl(fd, F_SETFL, 0);
    struct timeval rt = { 2, 0 };
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &rt, sizeof rt);
    char buffer[64];
    ssize_t n = read(fd, buffer, sizeof buffer - 1);
    if (n > 0) { buffer[n] = 0; printf("REDIRIGE -> %s", buffer); }
    else printf("connecte mais rien lu\n");
    return 0;
}
EOF
clang -O2 -o "$tmp/tera_tc" "$tmp/tera_tc.c" || { echo "compilation testclient echouee"; exit 1; }

python3 -c "
import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $SRV_PORT)); s.listen()
while True:
    c, _ = s.accept(); c.sendall(b'PF_OK\n'); c.close()
" &
srv=$!
sleep 0.5

echo "== etat pf actuel (sauvegarde) =="
was_enabled=$(sudo pfctl -s info 2>/dev/null | grep -c 'Status: Enabled')
echo "  pf active avant test: $was_enabled"

cat > "$tmp/tera_pf.conf" <<EOF
rdr pass proto tcp from any port != $EXCLUDE_SPORT to $FAKE_IP port $FAKE_PORT -> 127.0.0.1 port $SRV_PORT
EOF

echo "== chargement de la regle rdr (IP bidon $FAKE_IP, sans impact sur ton trafic) =="
sudo pfctl -E -f "$tmp/tera_pf.conf" 2>&1 | grep -iE 'token|enabled|error' | head

echo ""
echo "== TEST 1 : connexion normale vers $FAKE_IP:$FAKE_PORT =="
echo "   (attendu si pf gere le local: REDIRIGE -> PF_OK)"
"$tmp/tera_tc" "$FAKE_IP" "$FAKE_PORT"

echo ""
echo "== TEST 2 : meme cible mais depuis le port source $EXCLUDE_SPORT =="
echo "   (attendu: TIMEOUT -> l'exclusion casse-boucle marche)"
"$tmp/tera_tc" "$FAKE_IP" "$FAKE_PORT" "$EXCLUDE_SPORT"

echo ""
echo "== restauration pf =="
sudo pfctl -f /etc/pf.conf >/dev/null 2>&1
if [ "$was_enabled" = "0" ]; then sudo pfctl -d >/dev/null 2>&1; echo "  pf remis desactive"; else echo "  pf laisse active (comme avant)"; fi
kill $srv 2>/dev/null

echo ""
echo "=================================================="
echo "VERDICT :"
echo "  TEST1 = REDIRIGE  +  TEST2 = TIMEOUT  -> pf marche pour Wine, on construit."
echo "  TEST1 = TIMEOUT                        -> pf ne redirige pas le local, autre voie."
echo "=================================================="
