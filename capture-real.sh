#!/bin/sh
bottle="$HOME/Library/Application Support/CrossOver/Bottles/Tera"
temp="$bottle/drive_c/users/crossover/Temp"
wine=/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine
launcher='C:\Program Files\TERA Europe Classic+ Launcher\TERA Europe Classic+ Launcher.exe'

rm -f "$temp"/reply_*.bin

if ! ps aux | grep -q "[T]ERA Europe Classic+ Launcher"; then
    echo "demarrage du vrai launcher..."
    "$wine" --bottle Tera --cx-app "$launcher" >/dev/null 2>&1 &
fi

echo "en attente de la fenetre IPC — clique sur Play dans le launcher quand il est pret"
echo "(le jeu va se connecter au vrai serveur, c'est normal, tu peux le fermer des que l'ecran de serveurs s'affiche)"
found=0
attempt=0
while [ $attempt -lt 150 ]; do
    if "$wine" --bottle Tera --cx-app 'C:\users\crossover\Temp\tera-ipc-probe.exe' -- --list 2>/dev/null | grep -q "launcher window"; then
        found=1
        break
    fi
    attempt=$((attempt + 1))
    sleep 2
done

if [ $found -eq 0 ]; then
    echo "aucune fenetre LAUNCHER_CLASS apres 5 minutes, abandon"
    exit 1
fi

echo "fenetre trouvee, capture en cours"
"$wine" --bottle Tera --cx-app 'C:\users\crossover\Temp\tera-ipc-probe.exe' -- --events 5
echo
for f in "$temp"/reply_*.bin; do
    [ -e "$f" ] || continue
    echo "=== $(basename "$f") : $(wc -c < "$f") octets"
    cat "$f"; echo; echo
done
