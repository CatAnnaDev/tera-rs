import socket, time, sys

HOST, PORT, HOLD = "155.103.80.244", 9000, 35

t0 = time.monotonic()
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(HOLD)
try:
    s.connect((HOST, PORT))
except Exception as e:
    print(f"[{time.monotonic()-t0:5.1f}s] CONNECT KO : {e}")
    print("  -> port injoignable / refuse (firewall ou serveur down)")
    sys.exit(0)

print(f"[{time.monotonic()-t0:5.2f}s] TCP connecte, on tient la socket ouverte...")
try:
    data = s.recv(4096)
    dt = time.monotonic() - t0
    if data:
        print(f"[{dt:5.1f}s] RECU {len(data)} octets : {data[:16].hex()}")
        print("  -> JOIGNABLE (le serveur parle). Chemin ouvert.")
    else:
        print(f"[{dt:5.1f}s] EOF propre, 0 octet")
        print("  -> serveur a ferme sans rien envoyer (down/maintenance cote appli)")
except socket.timeout:
    print(f"[{time.monotonic()-t0:5.1f}s] pas de RST, socket tenue ouverte")
    print("  -> CHEMIN CLAIR : le serveur attend notre handshake. Vaut le coup de lancer le client.")
except ConnectionResetError:
    print(f"[{time.monotonic()-t0:5.1f}s] RST recu")
    print("  -> RST a 21s = middlebox. CET egress-ci est filtre (compare l'IP publique testee).")
except Exception as e:
    print(f"[{time.monotonic()-t0:5.1f}s] erreur : {e}")
finally:
    s.close()
