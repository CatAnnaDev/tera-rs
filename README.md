# tera-rs

A Rust workspace that reads, edits and rebuilds every bulk format of the TERA client, speaks
its network protocol, and runs a local test server — one set of crates behind a command line
(`tera`), a desktop app (`tera-studio`) and a handful of focused tools.

Written against, and verified on, a real install:

```
TERA Europe Classic — Live-100.02 GF #11 (Gold), client revision 376871, data revision 487849
UE3 package version 897, licensee 17, 64-bit data center format (version 6, revision 387463)
Asura private server (composite-packaged client)
```

## Build

```sh
cargo build --release
./target/release/tera --help          # command line
./target/release/tera-studio          # desktop app
```

`.cargo/config.toml` enables `--cfg aes_armv8` for the ARMv8 AES path used by the key scanner;
drop it on targets without those instructions. The Windows launcher stub builds with
`--target x86_64-pc-windows-gnu -p tera-launcher`.

## What works

| Area | Read | Write |
| --- | --- | --- |
| DataCenter (AES-128-CFB + deflate) | yes | yes, re-encrypted and reloadable |
| DataCenter ↔ XML / JSON | yes | XML/dir back to `.dat` |
| DataCenter attribute patching by query | yes | yes |
| AES key/IV recovery | files, dumps, live process | n/a |
| `.gpk` / `.upk` packages (LZO + zlib chunks) | yes | yes, rebuilt uncompressed |
| UE3 tagged properties | yes | per-property patch |
| `Texture2D` ↔ DDS/PNG, DXT1/3/5, BC4/BC5 | own decoder | own encoder |
| `SoundNodeWave` → Ogg Vorbis | yes | n/a |
| `StaticMesh` / `SkeletalMesh` → OBJ | yes | n/a |
| `PkgMapper.re` / `ObjectRedirectorMapper.re` / `DirCache.re` | yes | yes |
| `CompositePackageMapper.dat` (encrypted index) | yes | yes, byte-exact |
| Composite mod install / uninstall (the TMM workflow) | n/a | yes, reversible |
| Session crypto, packet framing, `.def` codec | yes | yes |
| Local login → character → zone | — | yes |

Measured on the reference install (Apple silicon, release build):

| Task | Time | Result |
| --- | --- | --- |
| Parse the 432 MB inflated EUR data center | 1.0 s | 4,415,750 nodes / 19,865,732 attributes |
| Export all 630 sheets to XML (1.5 GB) | 2.8 s | — |
| Rebuild + re-encrypt the data center | 11 s | 55,218,940 B vs 55,245,105 original |
| Index every package in `CookedPC` (9,603 files, 97 GB) | 15 s | 160,718 packages |
| Index every object | 21 s | 20,044,233 objects, 1.05 GiB mmap |
| Substring-search 20 M objects | 7–60 ms | index opens in 0.03 ms |
| Scan 146 MB of binaries for the DC key | 1.5 s | — |

One section per crate below, grouped by role. CLI crates document their own `tera` subcommands;
`<angle>` is required, `[square]` optional, `--flag` takes no value, `--opt <V>` takes one.

# Formats

## `tera-crypto`

AES-128-CFB (128-bit feedback, no padding), the built-in key table, and the parallel key/IV
scanner. The data center key changes with every client build and lives only inside the
WinLicense-packed executable, so it is recovered rather than read. Drives `tera keyfind`:

```
tera keyfind --data <dat> [targets...] [--pid N] [--mode adjacent|window|exhaustive] [--radius N] [--align N] [--loose] [--known]
```

The encrypted file is the oracle: a candidate must decrypt to a plausible size + valid zlib
header, then inflate. `adjacent` (default) assumes the IV sits next to the key; `window` tries
every IV within `--radius`; `exhaustive` every pair. `--pid` scans a live client (needs sudo).

## `tera-datacenter`

Reader, tree API, query engine, XML/JSON export, XML import and packer for
`DataCenter_Final_*.dat`. Drives `tera dc`; every command takes `--key <HEX32>` / `--iv <HEX32>`
to override the built-in key, or `--plain` for an already decrypted and inflated file.

```
dc info   <file>                                    header, key, counts
dc sheets <file>                                    top-level sheets and record counts
dc names  <file>                                    the name table
dc keys   <file>                                    key definitions and the nodes that use them
dc query  <file> <path> [--format xml|json|text] [--limit N] [--attribute NAME]
dc export <file> --out <dir> [--format xml|json|text] [--sheet NAME] [--merge]
dc unwrap <file> --out <bin>                        decrypted, inflated image
dc repack <file> --out <dat> [--plain-out] [--level N]
dc verify <file>                                    rebuild in memory, compare every count
dc set    <file> --select <query> [--set NAME=VALUE]... [--remove NAME]... --out <dat> [--dry-run] [--plain-out] [--level N]
dc pack   <dir>  --out <dat> [--template <dat>] [--key HEX] [--iv HEX] [--plain-out] [--level N]
```

Query syntax: `/Sheet/Node` walks children, `//Node` searches descendants, `*` any name,
`[@attr]` requires an attribute, `[@attr="value"]` compares its text, `[3]` selects by index.
`--template` on `pack` reuses the original name order, key definitions and attribute types, so
export → pack → export reproduces all 630 XML files byte for byte.

```sh
# patch a string in place, no XML
tera dc set "$DC" --select '/StrSheet_Item/String[@id="1"]' --set 'string=Midnight Rose Oil' --out out.dat
```

```rust
let dc = tera_datacenter::DataCenter::open(path)?;
for item in tera_datacenter::query(dc.root()?, "/ItemData/Item")? {
    let id = item.get("id").and_then(|v| v.as_i32());
}
```

## `tera-package`

UE3 packages end to end: summary, name/import/export tables, LZO and zlib chunk decompression,
tagged properties, `Texture2D`, `SoundNodeWave`, mesh recovery, package rebuild and authoring.
A `.gpk` is one or more complete packages laid end to end. Drives `tera gpk`:

```
gpk info            <file>                           sub-packages, offsets, compression
gpk list            <file> [--package N] [--class C] [-v]
gpk props           <file> <object> [--deep]         decoded UE3 tagged properties
gpk mips            <file> <object>                  Texture2D mip table
gpk materials       <file> [--filter TEXT] [--functions]
gpk gfx             <file> [--filter TEXT] [--out DIR]    GFx/Scaleform movies
gpk script          <file> [--filter TEXT]           UnrealScript bytecode
gpk extract         <file> --out <dir> [--raw] [--textures] [--png] [--sounds] [--filter TEXT]
gpk index           <root> --out <tsv> [--objects]   package name → file, offset, span
gpk mesh            <file> --object <name> --obj <out.obj>
gpk anim            <file> <object>
gpk map             <file>                           level / placement dump
gpk new-texture     <image> --package <name> --object <name> --out <gpk> [--format PF_DXT1|PF_DXT5] [--lod-group G]
gpk repack          <file> --out <gpk>               rebuild (uncompressed)
gpk replace-texture <file> --object <name> --dds <dds> --out <gpk>
gpk set-folder      <file> --folder <text> --out <gpk>   retag a package (e.g. OmiMapper mod)
```

Rebuilding always writes uncompressed (the LZO path has no encoder; the client reads
uncompressed packages fine). A `replace-texture` DDS must match the mip size and pixel format,
so the export keeps its exact length and the package keeps its size.

```sh
# replace an icon
tera gpk extract Icon_Status.gpk --out work/ --textures --filter FuriousBlows
magick work/icon.png -define dds:compression=dxt1 -define dds:mipmaps=0 work/new.dds
tera gpk replace-texture Icon_Status.gpk --object FuriousBlows --dds work/new.dds --out Icon_Status.gpk.new
# or author a package from an image
tera gpk new-texture my_art.png --package my_first_mod --object PinkBar_Tex --format PF_DXT5 --out my_first_mod.gpk
```

## `tera-mapper`

The mapper files the client uses to resolve packages: `PkgMapper.re`,
`ObjectRedirectorMapper.re`, `DirCache.re`, and the encrypted `CompositePackageMapper.dat` with
composite mod install/uninstall (the *Tera Mod Manager* mechanism, ported to Rust). Drives
`tera mapper`:

```
mapper pkg                 <file> [--grep TEXT] [--limit N]    PkgMapper.re
mapper redirects           <file> [--grep TEXT] [--limit N]    ObjectRedirectorMapper.re
mapper dircache            <file> [--grep TEXT] [--limit N]    DirCache.re
mapper composite           <file> [--grep TEXT] [--limit N]    CompositePackageMapper.dat
mapper composite-install   <mod.gpk> --game <CookedPC> [--name CONTAINER]
mapper composite-uninstall <target>  --game <CookedPC> [--delete-file]
mapper composite-restore             --game <CookedPC>
```

**Composite mods.** Live-service builds pack assets into shard files (`<hash>_NNN.gpk`) and
resolve objects through `CompositePackageMapper.dat`, mapping a composite name to a shard, an
offset and a length. A mod does not overwrite a base file: it ships a small replacement package
and repoints the index at it. The index is obfuscated with a 16-byte block permutation (a
self-inverse table), an odd/even pair swap and an XOR against `GeneratePackageMapper`;
`tera-mapper` round-trips the real 14.4 MB / 155,487-entry index byte for byte. A mod package is
an uncompressed UE3 package whose `folder_name` is tagged `OmiMapper: <composite>.<object>_dup`
— the client only substitutes tagged objects.

```sh
CP="…/S1Game/CookedPC"
tera gpk set-folder edited.gpk --folder "OmiMapper: ee54e3e4_b78ce296_15a00.PS.MonAggro_E_4993_PS_dup" --out mymod_300.gpk
tera mapper composite-install   --game "$CP" mymod_300.gpk --name mymod_300
tera mapper composite-uninstall --game "$CP" mymod_300 --delete-file
tera mapper composite-restore   --game "$CP"            # drop every mod, back to stock
```

Install backs the index up to `CompositePackageMapper.clean`, copies the package into
`CookedPC`, and repoints each sub-package it carries. Uninstall restores the original shard
route from that backup; no shard is rewritten, so it is fully reversible.

## `tera-index`

The memory-mapped asset index and its parallel search: fixed-size records plus one string
arena, object names interned and classes reduced to a small table (2.4 GiB → 1.05 GiB).
`Index::open` is a mmap and three integer reads; search is a parallel case-insensitive substring
scan. Drives `tera index`:

```
index build  <root> --out <idx> [--objects]          CookedPC → flat mmap index
index search <idx> <needle> [--objects] [--class C] [--limit N]
index info   <idx>
```

## `tera-mod`

Reversible, file-level mods: a `mod.toml` manifest, a backup of every file it touches under
`<game>/.tera-mods`, and receipts to revert. Distinct from the composite workflow above (this
edits base files in place). Drives `tera mod`:

```
mod new    <dir> [--name NAME]
mod show   <path>
mod plan   <path> --game <root> [--store DIR] [--datacenter <dat>]
mod apply  <path> --game <root> [--store DIR] [--datacenter <dat>]
mod revert <name> --game <root> [--store DIR]
mod status        --game <root> [--store DIR]
```

# Protocol & server

## `tera-protocol`

The wire protocol: session crypto (a SHA-0 expanded keystream feeding three voting shift
registers), packet framing, the opcode map loader, and the `.def` parser and codec. Handshake
is four bytes then client/server key pairs (128 B each); both sides shift+xor the four keys,
expand each through 34 SHA-0 passes, and seed shift registers of 55/57/58 words. Traffic is then
`[u16 size][u16 opcode][body]`, size including the header; the keystream is stateful across
unaligned writes. `.def` files carry `# majorPatchVersion >= N`, and selection picks, per
packet, the newest version whose range admits the current patch.

## `tera-world`

Shared game-state types used by the server and the hook mods: `Vec3`, `Entity`/`EntityKind`,
zones, and the system-message table. No binary of its own.

## `tera-server`

The local test server, built to stand the client up in a zone (login → create character →
Stepstone Isle → walk) rather than to emulate TERA. Binaries `tera-serverd` and `tera-encode`.

```sh
./target/release/tera-serverd \
  --opcodes ".../protocol.376012.map" \
  --definitions data/definitions --definitions data/definitions-classic --hex
```

| client | server |
| --- | --- |
| `C_CHECK_VERSION` | `S_CHECK_VERSION` |
| `C_LOGIN_ARBITER` | `S_LOGIN_ARBITER` (echoes the region) |
| `C_GET_USER_LIST` | `S_GET_USER_LIST` with the characters so far |
| `C_CREATE_USER` | `S_CREATE_USER` (keeps the appearance blobs) |
| `C_SELECT_USER` | `S_LOGIN`, `S_LOAD_TOPO`, `S_LOAD_HINT` |
| `C_LOAD_TOPO_FIN` | `S_SPAWN_ME` |
| — | `S_PING` every 10 s (the client drops after ~2 min without it) |

Characters persist in SQLite (`data/world.db`): accounts, characters, equipment, inventory,
learned skills, with cascading deletes. Creatures are a separate runtime population diffed
against each client's visible range (`C_SET_VISIBLE_RANGE`, 3000 here). Chat `!` commands and
the GM console's `C_ADMIN` cover movement (`!tp`, `!go`, `!zone`), spawning (`!npc`), gear
(`!equip`, `!gear`), stats (`!level`, `!heal`), and search (`!worlds`, `!npcs`, `!items`).
Unhandled packets are answered from `data/responses.json` or by `--auto-reply` (`C_X`→`S_X`).
Catalogues under `data/` (loaded in ~0.1 s): `worlds.json` (631), `npcs.json` (28,596),
`spawns.json` (45,733, the server-side placements that shipped in `TerritoryData`), `items.json`
(133,044), `skills.json` (8,387).

## `tera-launcher`

The Windows stub that runs inside the bottle and answers the three `WM_COPYDATA` questions the
client asks its launcher, pointing it at the local server. Binaries `tera-launcher`,
`tera-ipc-probe` (asks those three and prints the answers) and `tera-memdump`.

```sh
tera-launcher.exe --account 35171 --host 127.0.0.1 --port 10001 --game "C:\...\TERA.exe"
```

The client sends `WM_COPYDATA` to a `LAUNCHER_CLASS` window, `dwData` an event id: `1→2` account
name, `3→4` session ticket, `5→6` server list (protobuf), all UTF-16; it replies `1002` on
reject, `1003` on accept.

## `tera-hook`

The packet codec and hook/injection framework the proxy and the in-game mods share: a `Codec`,
a `Direction`, an `Action` (pass, drop, replace) and an `Injection`. No binary of its own; the
`mod-*` crates and `tera-proxy` build on it.

# Live tools

## `tera-proxy`

A MITM between client and server: decrypts both directions, logs one JSON packet per line, and
runs hooks.

```sh
tera-proxy --server host:port --log packets.jsonl
```

## `tera-relay`

A transparent TCP relay for TERA, egress direct or through a SOCKS5 proxy (`--socks user:pass`
for authenticated, e.g. residential, proxies).

## `tera-sniff`

A passive fake server: it performs the greeting and crypto handshake, then decodes in clear what
the client sends — useful for reading client packets without a real server.

## `tera-pcap`

Decodes a tcpdump capture of a session: it reads the clear handshake and decrypts everything
after it.

```sh
tera-pcap < capture.pcap
```

## `tera-replay`

Replays a JSONL capture: decodes each packet, verifies the codec round-trips, and rebuilds the
game-state. `--verbose` prints each decoded packet, `--strict` exits non-zero on any round-trip
failure.

## `tera-meter`

A native DPS meter that reads Noctenium's external-interface feed, fully local. `--demo`
populates it with fake data to judge the UI out of game.

## `tera-bot`

A headless client: handshake, login, character list, and automation. Resolves the game server
from the official server list when `--server` is absent.

## `tera-stats-tui`

A terminal view of the EU-Classic live-stats feed (`stats.tera-europe-classic.com`).

## `tera-account-tui`

A terminal front end for the EU-Classic account API (`tera-europe-classic.com/api`).

# Apps & mods

## `tera-cli`

The `tera` binary — every `dc`, `gpk`, `mapper`, `index` and `mod` command above, plus:

```
server-data npc-skills | npcs | villagers       official datasheets → server catalogues
dump   --pid <N> [--out DIR] [--executable] [--min-size N] [--images]
disasm <file> [--base ADDR] [--out FILE] [--limit N] [--skip-filler]
```

## `tera-ui`

`tera-studio`, the egui desktop app — a front end over the same crates, so everything it does is
also a `tera` subcommand. The asset browser filters 20 million objects live and decodes the
selection: `Texture2D` to an image (export PNG/DDS, replace pixels, author a new mod package),
meshes to a software-rendered 3D view and OBJ, `SoundNodeWave` to a waveform and Ogg, anything
else to its property table. It also opens a data center for query/edit/export, browses the
mapper files, and runs the key scanner. Deep links drive and test it:

```sh
tera-studio --query LoadingProgress --class Texture2D --pick 0
tera-studio --dc-query '/StrSheet_Item/String[@id="1"]'
```

## `mod-radar`

A hook mod that tracks `S_SPAWN_*` / `S_LOAD_TOPO` and shows an entity radar. Built on
`tera-hook` + `tera-world` + `tera-protocol`.

## `mod-antiafk`

A hook mod that keeps the session alive.

## `mod-example`

A minimal hook-mod template to copy from.

# Caveats

* Rebuilding a package drops its compression, so files grow on disk.
* A rebuilt data center follows name-table order; adding records means inserting them in the
  client's key order, editing existing ones is safe.
* Bulk data stored outside the package (`.tfc`) is reported, not extracted.
* A patched client data center can disagree with the server about the data revision.
* Mesh recovery is heuristic: geometry only, not material assignment or LODs.

# Format notes

`docs/formats.md` has the on-disk layouts, including the parts specific to TERA rather than
stock UE3.
