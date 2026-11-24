# tera-scanner

Live memory scanner for the running 64-bit TERA client. Reads the client process
from outside with `ReadProcessMemory` (no DLL injection) and extracts static tables.
Replaces the abandoned 32-bit TeraScanners, which cannot target a 64-bit client.

## Build

Cross-compiled to a Windows binary and run inside the same wine bottle as the client.

```
rustup target add x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build -p tera-scanner --release --target x86_64-pc-windows-gnu
```

Needs `x86_64-w64-mingw32-gcc` on `PATH` (e.g. `brew install mingw-w64`).

## Run

Copy the binary into the bottle and run it there while the client is in-game:

```
cp target/x86_64-pc-windows-gnu/release/tera-scanner.exe \
  "~/Library/Application Support/CrossOver/Bottles/tera/drive_c/windows/temp/"
CX_BOTTLE=tera WINEDEBUG=-all \
  "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine" \
  'C:\windows\temp\tera-scanner.exe' <command>
```

## Commands

| command | effect |
| --- | --- |
| `regions` | list committed regions (executable and large ones) |
| `find <hex>` | search executable regions for a hex byte pattern |
| `find-all <hex>` | search every readable region for a hex byte pattern |
| `findw <text>` | search every readable region for a UTF-16 string |
| `dump <addr> <len> <out>` | dump a byte range to a file, tolerating unreadable pages |
| `sysmsg [out]` | extract the system-message id to name table as a toolbox map |

`sysmsg` anchors on the `SMT_UNDEFINED` to `SMT_MAX` pointer array inside the client
module, so it keeps working across client patches without hard-coded addresses.

## What cannot be extracted here

Opcode name strings are stripped from this client and it has no name lookup function,
so an opcode map cannot be dumped from memory. Use the maintained `protocol.<ver>.map`.
The datacenter key and IV are handled by `tera-datacenter` / `tera-crypto`.
