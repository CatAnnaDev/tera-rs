# tera-rs

A Rust toolkit that reads, edits and rebuilds every bulk data format shipped with the
TERA client: the encrypted `DataCenter_Final_*.dat` database, the `CookedPC` `.gpk`
Unreal Engine 3 packages, and the `.re` mapper files that glue them together.

Everything here was written against, and verified on, a real install:

```
TERA Europe Classic — Live-100.02 GF #11 (Gold), client revision 376871, data revision 487849
UE3 package version 897, licensee 17, 64-bit data center format (version 6, revision 387463)
```

`tera-studio` is the desktop app; `tera` is the command line. Both sit on the same crates.

## What works

| Area | Read | Write |
| --- | --- | --- |
| DataCenter (AES-128-CFB + deflate) | yes | yes, re-encrypted and reloadable |
| DataCenter → XML / JSON | yes | XML back to `.dat` |
| DataCenter attribute patching by query | yes | yes |
| AES key/IV recovery | scan files, dumps or a live process | n/a |
| `.gpk` / `.upk` packages (LZO and zlib chunks) | yes | yes, rebuilt uncompressed |
| Package bundles (many packages per file) | yes | yes |
| UE3 tagged properties | yes | n/a |
| `Texture2D` → DDS | yes | DDS back into the package |
| `SoundNodeWave` → Ogg Vorbis | yes | n/a |
| `PkgMapper.re`, `ObjectRedirectorMapper.re`, `DirCache.re` | yes | yes |
| `CompositePackageMapper.dat` (encrypted composite index) | yes | yes, re-encrypted byte-exact |
| Composite mod install / uninstall (the TMM workflow) | n/a | yes, reversible from a backup |
| `StaticMesh` / `SkeletalMesh` geometry | yes, exports Wavefront OBJ | n/a |
| Brand new mod packages built from a PNG or DDS | n/a | yes |
| DXT1/DXT3/DXT5/BC4/BC5 and PNG | own decoder | own encoder |

Measured on the reference install (Apple silicon, release build):

* load + parse the 432 MB inflated EUR data center: **1.0 s** (4,415,750 nodes, 19,865,732 attributes)
* export all 630 sheets to XML (1.5 GB): **2.8 s**
* rebuild the whole data center and re-encrypt it: **11 s**, output 55,218,940 bytes vs 55,245,105 original
* index every package in `CookedPC` (9,603 files, 97 GB): **15 s** → 160,718 packages, 20,044,233 objects
* scan 146 MB of binaries for the data center key: **1.5 s**
* index every object in `CookedPC`: **21 s** → 20,044,233 objects in a 1.05 GiB mmap-able file
* search those 20 million objects by substring: **7–60 ms**, and the index opens in **0.03 ms**

## Build

```sh
cargo build --release
./target/release/tera --help          # command line
./target/release/tera-studio          # desktop app
```

`.cargo/config.toml` enables `--cfg aes_armv8`, which turns on the ARMv8 AES instructions
used by the key scanner. Drop it if you build for a target without them.

## The app

`tera-studio` opens on the asset browser. Everything it does is also a `tera` subcommand, so
the app is a front end, not a second implementation.

**assets** — type in the search box to filter 20 million objects live, optionally narrowed to
one class. Selecting an object decodes it and shows what it actually is:

* `Texture2D` → the decoded image over an alpha checkerboard, with format, size and mip count,
  and buttons to export PNG or DDS, replace the pixels with your own image, or spin a brand new
  mod package out of an image.
* `StaticMesh` / `SkeletalMesh` → a software-rendered 3D view (drag to orbit, scroll to zoom)
  and an OBJ export.
* `SoundNodeWave` → the decoded waveform, a play button, and an Ogg export.
* anything else → the full UE3 tagged-property table and a raw object export.

The first run has no index; the button builds one (about 30 s for the whole `CookedPC`) into
`~/.tera-studio/assets.idx`.

**datacenter** — opens a `DataCenter_Final_*.dat`, lists its 630 sheets with record counts,
runs path queries, shows a node's attributes, queues edits, and writes a patched, re-encrypted
`.dat`. Also exports XML or JSON.

**mods** — lists the mod packages installed in the launcher's mods folder, browses and edits
`PkgMapper.re`, and writes it back with a `.bak` next to it.

**keys** — runs the key/IV scanner with a progress bar over a file, a folder or the built-in
key table.

The window takes deep links, which is also how it gets tested:

```sh
tera-studio --query LoadingProgress --class Texture2D --pick 0
tera-studio --tab mods
tera-studio --dc-query '/StrSheet_Item/String[@id="1"]'
```

## Asset index

```sh
tera index build CookedPC --out assets.idx --objects
tera index search assets.idx crosshair --objects --class StaticMesh
tera index info assets.idx
```

The index is a flat, memory-mapped file: fixed-size records plus one string arena, with object
names interned and class names reduced to a small table, which is what takes it from 2.4 GiB to
1.05 GiB. Nothing is parsed at open time, so `Index::open` is a mmap and three integer reads.
Searching is a parallel case-insensitive substring scan over the arena.

## Data center

```sh
DC="…/S1Game/S1Data/DataCenter_Final_EUR.dat"

tera dc info    "$DC"                 # header, key, node/attribute/string counts
tera dc sheets  "$DC"                 # the 630 top level sheets and their record counts
tera dc keys    "$DC"                 # key definitions and which nodes use them
tera dc query   "$DC" '/ItemData/Item[@id="1"]'
tera dc query   "$DC" '/StrSheet_Item/String[@id="1"]' --format text
tera dc export  "$DC" --out out/ --merge          # one XML per sheet
tera dc export  "$DC" --out out/ --format json    # one JSON file per sheet node
tera dc unwrap  "$DC" --out plain.bin             # decrypted and inflated image
```

Editing, without ever touching XML:

```sh
tera dc set "$DC" \
  --select '/StrSheet_Item/String[@id="1"]' \
  --set 'string=Huile de Minuit Rose' \
  --out DataCenter_Final_EUR.dat
```

Or the full round trip through XML:

```sh
tera dc export "$DC" --out xml/ --merge
$EDITOR xml/StrSheet_Item.xml
tera dc pack xml/ --out DataCenter_Final_EUR.dat --template "$DC"
```

`--template` reuses the original name table order, key definitions and attribute types, so
the rebuilt file keeps the client's lookup structure and does not turn the string `"00"`
into the integer `0`. Exporting all 630 sheets, packing them back and exporting again
reproduces all 630 XML files byte for byte.

`tera dc verify "$DC"` rebuilds in memory and compares node, attribute, name, value and
per-sheet counts against the original.

### Query syntax

`/Sheet/Node` walks children, `//Node` searches descendants, `*` matches any name,
`[@attr]` requires an attribute, `[@attr="value"]` compares its text form, and `[3]`
selects by index.

## Key and IV

The data center is AES-128 in CFB mode (128-bit feedback, no padding). Key and IV change
with every client build and live only inside the executable, which is packed with
WinLicense on the retail client, so they cannot be read statically from `TERA.exe`.

The key for the reference client is built in and detected automatically:

```
key 1C01C904FF76FF06C211187E197B5716
iv  396C342C52A0C12D511DD0209F90CA7D
```

`tera keyfind` recovers an unknown one. It uses the encrypted file itself as the oracle:
decrypting the first block must yield a plausible uncompressed size followed by a valid
zlib header, and surviving candidates must inflate.

```sh
tera keyfind --data "$DC" --known                        # try the built-in table first
tera keyfind --data "$DC" ./dumps ./Binaries             # scan files and directories
tera keyfind --data "$DC" --pid 1632                     # scan a running client (needs sudo)
tera keyfind --data "$DC" --mode window --radius 256 dump.bin
tera keyfind --data "$DC" --mode exhaustive small.bin
```

`adjacent` (the default) assumes the IV sits directly before or after the key, which is how
the client stores them, and costs two AES blocks per offset. `window` tries every IV within
`--radius` bytes of the key. `exhaustive` tries every pair. `--align 4` or `--align 16` cuts
the work when the constants are aligned. If `task_for_pid` is refused, dump the process
first:

```sh
lldb -p <pid> -o "process save-core dump.core" -o detach -b
tera keyfind --data "$DC" dump.core
```

## Packages

A `.gpk` file is one *or more* complete UE3 packages laid end to end. `17d87899_1.gpk` for
example holds 208 packages whose spans add up to exactly its 133,830,733 bytes.

```sh
tera gpk info   CookedPC/17d87899_1.gpk          # every sub-package, offsets, compression
tera gpk list   CookedPC/…/Icon_Status.gpk -v    # exports with class, outer, flags
tera gpk list   file.gpk --class Texture2D
tera gpk props  file.gpk FuriousBlows            # decoded UE3 tagged properties
tera gpk extract file.gpk --out out/ --textures --sounds --raw
tera gpk index  CookedPC --out index.tsv         # package name → file, offset, span
```

`index.tsv` is what turns a `PkgMapper` entry such as
`Q_MISTEL_ANI.ANIM.Q_MISTEL_ANIM → 17d87899_101850eb_7a91.Anim.Q_Mistel_Anim_dup`
into a file on disk, because the package name never appears in the file name.

### Creating a mod package from scratch

```sh
tera gpk new-texture my_art.png --package my_first_mod --object PinkBar_Tex \
  --format PF_DXT5 --out my_first_mod.gpk
```

This writes a complete UE3 package with the same three exports the game's own mod packages
use — an `ObjectReferencer`, a `Package` marker carrying the package name, and the `Texture2D`
itself. `--format PF_A8R8G8B8` stores the image uncompressed, which round-trips bit for bit.

The DXT encoder fits endpoints on the block's principal axis, refines them by least squares,
then hill-climbs in 565 space. On the reference texture it lands at RMSE 65.8 against
ImageMagick's 47.3, from 250 for a plain bounding-box fit, and encodes a 2048×128 texture in
30 ms.

### Meshes

```sh
tera gpk mesh package.gpk --object ace_4560_sl_dup --obj mesh.obj
```

Mesh recovery scans for the vertex buffer header, validates it against an index buffer and
drops degenerate triangles, so it does not depend on the exact licensee layout. It came from
Hysteria Studio, along with the software rasterizer the app uses to draw the result.

### Replacing a texture

```sh
tera gpk extract Icon_Status.gpk --out work/ --textures --filter FuriousBlows
magick work/*.dds work/icon.png
# edit icon.png
magick work/icon.png -define dds:compression=dxt1 -define dds:mipmaps=0 work/new.dds
tera gpk replace-texture Icon_Status.gpk --object FuriousBlows --dds work/new.dds \
  --out Icon_Status.gpk.new
```

The new DDS must match the mip's size and pixel format, so the patched export keeps its
exact length and the rebuilt package stays the same size as the original. Rebuilding a
package always writes it uncompressed: the LZO decompressor here has no encoder, and the
client reads uncompressed packages fine — most of `CookedPC` already ships that way. A
package that was already uncompressed rebuilds byte for byte identical.

## Mapper files

```sh
tera mapper pkg       S1Game/PkgMapper.re --grep S1UI
tera mapper redirects S1Game/ObjectRedirectorMapper.re
tera mapper dircache  S1Game/DirCache.re
```

`tera-mapper` also writes these files back, which is how a mod registers a replacement
package with the client.

## Composite package mods (the TMM workflow)

Live service builds of TERA (Asura and other private shards) do not load most assets from
their own `.gpk` files directly. They pack everything into a handful of large shard files
(`<hash>_NNN.gpk`) and resolve objects through one encrypted index,
`CookedPC/CompositePackageMapper.dat`, which maps a composite name to the shard file, the
byte offset inside it and the length. A mod does not overwrite a base file; it ships a small
replacement package and repoints that index at it. This is what *Tera Mod Manager* does, now
ported into `tera-mapper` and the `tera` binary.

`CompositePackageMapper.dat` is obfuscated, not strongly encrypted: a 16-byte block
permutation (whose table is its own inverse), a swap of the odd/even byte pairs around the
middle, and an XOR against the ASCII key `GeneratePackageMapper`. `tera-mapper` reproduces all
three and round-trips the real 14.4 MB index byte for byte across all 155,487 entries.

Read or search the index:

```sh
CP="…/S1Game/CookedPC"
tera mapper composite "$CP/CompositePackageMapper.dat" --grep MonAggro
```

Each line is `container  composite-name  object-path  off N size N`. A base route names a
shard (`ee54e3e4_171`); an installed mod names its own file.

A mod package is an ordinary UE3 package, stored uncompressed, whose `folder_name` is tagged
`OmiMapper: <composite>.<object>_dup`. The client recognises that tag and substitutes the
object; an untagged package at the same route is ignored, so the tag is not optional. Tag a
package you have already edited (for instance after `gpk replace-texture` or a material
recolor) with:

```sh
tera gpk set-folder edited.gpk \
  --folder "OmiMapper: ee54e3e4_b78ce296_15a00.PS.MonAggro_E_4993_PS_dup" \
  --out mymod_300.gpk
```

Install it. This backs the index up to `CompositePackageMapper.clean` the first time (and to
`CompositePackageMapper.dat.bak` every time), copies the package into `CookedPC` under its
container name, reads every `OmiMapper:` sub-package it contains, and repoints each composite
at it:

```sh
tera mapper composite-install --game "$CP" mymod_300.gpk --name mymod_300
```

Undo one mod, which restores the original shard route for each composite it overrode from the
`.clean` backup, optionally deleting the copied file:

```sh
tera mapper composite-uninstall --game "$CP" mymod_300 --delete-file
```

Or drop every mod at once and return the index to stock:

```sh
tera mapper composite-restore --game "$CP"
```

Because an install never rewrites a shard and the base routes are kept in `.clean`, the whole
operation is reversible without reinstalling the client.

## Crates

* `tera-crypto` — AES-128-CFB, the known-key table, and the parallel key/IV scanner.
* `tera-datacenter` — reader, tree API, query engine, XML/JSON export, XML import, packer.
* `tera-package` — UE3 summary, name/import/export tables, LZO and zlib chunk
  decompression, tagged properties, `Texture2D`, `SoundNodeWave`, package rebuilding.
* `tera-mapper` — `PkgMapper.re`, `ObjectRedirectorMapper.re`, `DirCache.re`, and the
  encrypted `CompositePackageMapper.dat` with composite mod install/uninstall.
* `tera-index` — the memory-mapped asset index and its parallel search.
* `tera-cli` — the `tera` binary.
* `tera-ui` — the `tera-studio` desktop app (egui).

Library use:

```rust
let dc = tera_datacenter::DataCenter::open(path)?;
for item in tera_datacenter::query(dc.root()?, "/ItemData/Item")? {
    let id = item.get("id").and_then(|value| value.as_i32());
    let name = item.get("name").map(|value| value.to_text());
}
```

## Command reference

Everything the `tera` binary exposes. `<angle>` is a required positional argument, `[square]`
is optional, `--flag` takes no value, `--opt <V>` takes one. Run any command with `--help` for
the exact wording.

### `tera dc` — the DataCenter database

Every `dc` command takes `--key <HEX32>` and `--iv <HEX32>` to override the built-in key, or
`--plain` when the file is already decrypted and inflated.

```
dc info    <file>                                   header, key, node/attribute/string counts
dc sheets  <file>                                   top-level sheets and their record counts
dc names   <file>                                   the name table
dc keys    <file>                                   key definitions and the nodes that use them
dc query   <file> <path> [--format xml|json|text] [--limit N] [--attribute NAME]
dc export  <file> --out <dir> [--format xml|json|text] [--sheet NAME] [--merge]
dc unwrap  <file> --out <bin>                        decrypted, inflated image
dc repack  <file> --out <dat> [--plain-out] [--level N]
dc verify  <file>                                   rebuild in memory and compare every count
dc set     <file> --select <query> [--set NAME=VALUE]... [--remove NAME]... --out <dat>
           [--plain-out] [--level N] [--dry-run]
dc pack    <dir>  --out <dat> [--template <dat>] [--key HEX] [--iv HEX] [--plain-out] [--level N]
```

### `tera keyfind` — recover the DataCenter key and IV

```
keyfind --data <dat> [targets...] [--pid N] [--mode adjacent|window|exhaustive]
        [--radius N] [--align N] [--loose] [--known]
```

`--data` is the encrypted file used as the oracle; targets are files, folders or raw dumps to
scan. `--known` tries the built-in table first; `--pid` scans a live process (needs sudo).

### `tera gpk` — CookedPC packages

```
gpk info            <file>                           sub-packages, offsets, compression
gpk list            <file> [--package N] [--class C] [-v]
gpk props           <file> <object> [--deep]         decoded UE3 tagged properties
gpk mips            <file> <object>                  mip table of a Texture2D
gpk materials       <file> [--filter TEXT] [--functions]
gpk gfx             <file> [--filter TEXT] [--out DIR]     GFx/Scaleform movies
gpk script          <file> [--filter TEXT]           UnrealScript / bytecode
gpk extract         <file> --out <dir> [--raw] [--textures] [--png] [--sounds] [--filter TEXT]
gpk index           <root> --out <tsv> [--objects]   package name -> file, offset, span
gpk mesh            <file> --object <name> --obj <out.obj>
gpk anim            <file> <object>
gpk map             <file> ...                        level / placement dump
gpk new-texture     <image> --package <name> --object <name> --out <gpk>
                    [--format PF_DXT1|PF_DXT5] [--lod-group G]
gpk repack          <file> --out <gpk>               rebuild (uncompressed)
gpk replace-texture <file> --object <name> --dds <dds> --out <gpk>
gpk set-folder      <file> --folder <text> --out <gpk>    tag a package (e.g. OmiMapper: ...)
```

### `tera mapper` — mapper files and composite mods

```
mapper pkg                 <file> [--grep TEXT] [--limit N]     PkgMapper.re
mapper redirects           <file> [--grep TEXT] [--limit N]     ObjectRedirectorMapper.re
mapper dircache            <file> [--grep TEXT] [--limit N]     DirCache.re
mapper composite           <file> [--grep TEXT] [--limit N]     CompositePackageMapper.dat
mapper composite-install   <mod.gpk> --game <CookedPC> [--name CONTAINER]
mapper composite-uninstall <target>  --game <CookedPC> [--delete-file]
mapper composite-restore             --game <CookedPC>
```

`<target>` for uninstall is either a composite name or the container filename.

### `tera index` — the memory-mapped asset index

```
index build  <root> --out <idx> [--objects]          CookedPC -> flat mmap index
index search <idx> <needle> [--objects] [--class C] [--limit N]
index info   <idx>
```

### `tera mod` — the manifest mod system (`mod.toml`)

A separate, reversible way to edit base files in place, backing each one up under
`<game>/.tera-mods`. Distinct from the composite workflow above.

```
mod new    <dir> [--name NAME]                       write a commented example manifest
mod show   <path>                                    what a manifest declares
mod plan   <path> --game <root> [--store DIR] [--datacenter <dat>]
mod apply  <path> --game <root> [--store DIR] [--datacenter <dat>]
mod revert <name> --game <root> [--store DIR]
mod status        --game <root> [--store DIR]
```

### `tera server-data` — import official datasheets

```
server-data npc-skills ...        NpcSkillData  -> server skill catalogue
server-data npcs       ...        NpcData       -> creature catalogue
server-data villagers  ...        VillagerData  -> fixed-post npcs
```

### `tera dump` / `tera disasm` — binary spelunking

```
dump   --pid <N> [--out DIR] [--executable] [--min-size N] [--images]
disasm <file> [--base ADDR] [--out FILE] [--limit N] [--skip-filler]
```

## Caveats

* Rebuilding a package drops its compression, so files get larger on disk.
* Sibling order inside a rebuilt data center follows the name table order. Adding new
  records means inserting them in the same key order the client expects; editing existing
  ones is safe.
* Bulk data stored outside the package (`.tfc` style) is reported but not extracted.
* A patched client data center can disagree with the server about the data revision.
* Mesh recovery is heuristic: it finds the geometry, not the material assignment or the LODs.
* The app needs the asset index before it can browse; building it reads every package header.

## Format notes

See `docs/formats.md` for the on-disk layouts, including the parts that are specific to
TERA rather than stock UE3.

## Local test server

The point of the server is not to emulate TERA, it is to get the client standing in a zone so
client-side mods can be exercised. It does that: the client logs in, creates a character, picks
it, loads Stepstone Isle and walks around. Three pieces:

* `tera-protocol` — SHA-0, the three-register stream cipher, the session key exchange, packet
  framing, the opcode map loader, the `.def` parser and a definition-driven packet
  reader/writer.
* `tera-serverd` — the TCP side: greeting, key exchange, decryption, dispatch, and the lobby
  and spawn flow.
* `tera-launcher` — a Windows stub that runs inside the bottle and answers the three
  `WM_COPYDATA` questions the client asks its launcher, pointing it at the local server.

`./run-local.sh` builds all three, starts the server, and launches the game through the stub.
By hand:

```sh
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu -p tera-launcher

./target/release/tera-serverd \
  --opcodes "…/classicplus.tcc/resources/data/opcodes/protocol.376012.map" \
  --definitions data/definitions --definitions data/definitions-classic --hex
```

Then, with the real launcher closed, from the bottle:

```
tera-launcher.exe --account 35171 --host 127.0.0.1 --port 10001 \
  --game "C:\Games\TERA Europe Classic\Binaries\TERA.exe"
```

### Picking the opcode map

The opcode map has to match the client build, and the data center revision is **not** the
protocol revision. `DataCenter_Final_EUR.dat` is revision 387463, but this client negotiates
375500/375489 in `C_CHECK_VERSION` and its opcodes are the ones in `protocol.376012.map`
(1746 entries). The 387463 map ClassicPlus also ships holds only the 415 opcodes that mod
needs, and its `C_LOGIN_ARBITER` is a different build's — using it makes login arrive as
`UNKNOWN_23285`. `C_CHECK_VERSION` is 19900 in every map, so a login that negotiates the
version and then goes quiet means the wrong map.

### Definition folders

`--definitions` may be repeated; later folders win regardless of version. `data/definitions`
holds the current tera-toolbox set, `data/definitions-classic` pins the versions this client
actually speaks (`S_GET_USER_LIST.15`, `S_LOGIN.12`) plus `S_LOAD_HINT.1`, which upstream does
not carry.

### What the server answers

| client | server |
| --- | --- |
| `C_CHECK_VERSION` | `S_CHECK_VERSION` |
| `C_LOGIN_ARBITER` | `S_LOGIN_ARBITER`, echoing the client's region |
| `C_GET_USER_LIST` | `S_GET_USER_LIST` with the characters created so far |
| `C_CAN_CREATE_USER` | `S_CAN_CREATE_USER` |
| `C_CHECK_USERNAME` | `S_CHECK_USERNAME` |
| `C_CREATE_USER` | `S_CREATE_USER`, keeping the appearance blobs the client sent |
| `C_SELECT_USER` | `S_LOGIN`, `S_LOAD_TOPO`, `S_LOAD_HINT` |
| `C_LOAD_TOPO_FIN` | `S_SPAWN_ME` |
| — | `S_PING` every ten seconds; the client drops the session after about two minutes without it |

Characters live in memory only and are lost when the server stops. Spawn is zone 5, Stepstone
Isle, at `(16260, 1253, -4410)`.

### Persistence

Everything a character owns lives in a SQLite file, `data/world.db` by default, created on first
run. Tables: `accounts`, `characters`, `equipment`, `inventory`, `learned_skills`, with foreign
keys and cascading deletes. Position, facing, level, experience, gold, worn items and learned
skills all survive a restart; the character reappears where it logged out rather than at the
spawn point. `--database` and `--account` pick the file and the owner.

### The living world

`realm.rs` holds the creatures that currently exist, keyed by zone. It is deliberately separate
from the database: characters persist, creatures are a runtime population.

Visibility is diffed rather than broadcast. When the player moves more than 200 units, or when a
command changes the population, the server compares what is in range against what that connection
has already been told and sends only the difference — `S_SPAWN_NPC` for arrivals, `S_DESPAWN_NPC`
for departures. The range is not guessed: the client announces it with `C_SET_VISIBLE_RANGE`
(3000 on this install).

Creatures are placed at the player's own height. The server has no terrain, and `S_SPAWN_NPC`
renders a creature exactly where it is told, so the only height known to be on the ground is the
one the player is standing on. `!npc` puts a creature 150 units ahead on that same plane, `!npc …
here` at the player's feet.

### Definition versions

`.def` files carry a `# majorPatchVersion >= N` header, and `--patch-version` (default 100)
picks, per packet, the newest version whose range admits it. That single number is what makes
the difference between a character that renders and one that does not: `S_LOGIN.12` and `.14`
differ by eight bytes and a trailing array, so sending the wrong one shifts every field after
it — appearance included. This client asks for servant lists, which puts it at 86 or later, and
`S_GET_USER_LIST.15` is accepted where `.21` (104 and up) is not, so 100 is the working value.
`data/definitions` holds every version from the current tera-toolbox set, its 2021 snapshot and
the classic set, and selection sorts them out. `--pin S_CHAT=4` forces one packet to a chosen
version, which is the quick way to test a suspect without touching the corpus.

Two definitions in that folder are ours, reconstructed from bytes the client actually sent
because no published version fits this build:

* `C_REQUEST_CONTRACT.2` — the shipped `.1` puts its references first, the client puts two
  `int32` first. Two captures of the same packet, compared byte by byte, gave the real order; the
  two fields that differ between captures move by the same delta, which is what identified them
  as counters. A test decodes the real 32-byte body.
* `S_SPAWN_NPC.11` — the only published version is `.12`, headed `majorPatchVersion >= 101`,
  and this client is 100. `level`, `maxHp` and `enrageThreshold` were added in 101; sending them
  shifts `templateId` and `huntingZoneId` 20 bytes out of place, the model lookup fails and
  **nothing is drawn at all** — no error, no complaint, just an empty world. Removing those three
  fields makes creatures appear. A test asserts that patch 100 selects `.11`, patch 110 selects
  `.12`, and the difference is exactly 20 bytes.

That is the general shape of the work: a packet that renders nothing is far more often a version
mismatch than a bug in the encoder.

### Checked against the original parser

The rules above were reverse engineered before the reference implementation was to hand. It is
now checked against it directly, by running tera-toolbox's `tera-data-parser` under node and
having it **read back the bytes our writer produces**, for every definition in the corpus.

Interoperability, not byte equality, is the test that matters, and the distinction earned its
keep: a static reading of the reference concluded that our `S_LOGIN` was broken, while running it
showed the opposite. The reference writes a string or an array where its field is declared and we
write it at the end of the level, so the bytes differ — but every offset is patched to match, so
each side reads the other perfectly.

835 of 891 definitions round-trip. Of the rest:

* 54 differ only in **units**. The reference exposes `angle` and `vec3fa` in radians and converts
  to a scaled `i16` on the wire; we work in raw wire units throughout. Same bytes, different API.
* One, `I_INPUT_COMMAND.1`, is a place where the reference disagrees with itself — write a packet
  with its own writer and its own reader gives back a different `unk`.

Three real bugs surfaced this way and are fixed:

* `array<vec3>[interleaved]` parsed as the subtype `vec3>[interleaved]`, so the element type was
  lost and the array silently degraded to empty objects. Subtype and flags are separate tokens.
* Typed arrays always wrote four bytes per element, so `array<uint64>` dropped the top half of
  every value and `array<vec3>` wrote zeros where the coordinates should be.
* `object` blocks were treated as arrays, so their bytes were neither read nor written. Getting
  them right needs two things the shape of the file does not make obvious: a reference declared
  inside an object attaches to the nearest enclosing **non-object** level under a dotted key like
  `owner.name`, and two `object` blocks with the same name at one level are one object. That fix
  is what makes `S_EACH_SKILL_RESULT` encodable, so combat depends on it.

One gap is known and deliberate. `skillid` is not a number: it packs an id, a hunting zone, a
type nibble and two flags into 64 bits. `value::SkillId` composes and decomposes it —
`SkillId::player(id)` and `SkillId::creature(id, zone)` — but the codec still moves the field as
a plain `uint64`, so a caller has to pass `.raw()`. The same is true of `customize`.

### In-game commands

Anything typed in chat starting with `!` is run by the server, as is the free-text `C_ADMIN`
packet the client's GM console sends. TERA does not ship a GM vocabulary — `C_ADMIN` is a bare
`string command`, so the server defines it:

| command | effect |
| --- | --- |
| `!help` | list the commands |
| `!where` | current zone, position and facing |
| `!tp x y z` | move within the zone (`S_INSTANT_MOVE`) |
| `!go name` | jump to a named world position |
| `!worlds text` | search the world positions |
| `!zone id [x y z]` | load another topology |
| `!speed run [walk]` | change movement speed |
| `!level n` | set the level and refresh stats |
| `!hp n` / `!heal` | refresh stats, optionally with a given hp |
| `!npc name\|id [huntingZone]` | spawn a creature beside you |
| `!npcs text` | search the creatures |
| `!equip slot name\|id` | equip an item (`weapon body hand feet underwear head face`, or a slot number) |
| `!items text` | search the items |
| `!gear` | list what is equipped |
| `!gm on\|off` | set the admin level shown in the lobby |
| `!say text` | send yourself a chat line |
| `!ch n` | pick the channel replies go to |
| `!chtest` | send one line on every channel through `S_CHAT` |
| `!msgtest` | send one line through four different server packets |
| `!markup on\|off` | toggle the colour markup on replies |

Position and facing are tracked from `C_PLAYER_LOCATION`, so `!tp` and `!npc` work relative to
where you actually are.

### The catalogues

Five tables under `data/`, all loaded in about a tenth of a second:

| file | rows | from | used by |
| --- | --- | --- | --- |
| `worlds.json` | 631 | `AutoWorldProfile` | `!go`, `!worlds` |
| `npcs.json` | 28,596 | `NpcData/Template` joined to `NpcShape` and the mod's English names | `!npc`, `!npcs` |
| `spawns.json` | 45,733 | `TerritoryData` | `!populate` |
| `items.json` | 133,044 | `ItemData` joined to `StrSheet_Item` | `!equip`, `!items` |
| `skills.json` | 8,387 | ClassicPlus `skills-EU-EN.tsv` | `!skills` |

`TerritoryData` is the one worth knowing about. TERA's creature placements were server-side data
and are usually assumed lost, but the sheet shipped: every territory carries a `Fence` polygon
and the `Npc` templates that belong to it, which is 45,733 placements with real ground heights
and roam radii. It cross-checks against `StrSheet_NpcLoc` to a median of 8 world units. 43 rows
name a hunting zone with no continent and are skipped, which the log states rather than failing
on.

Searching is shared: an exact name wins, then a name that starts with the fragment, then one
that contains it, all case-insensitive and without allocating per row.

Each table carries what its packet needs, not just a label. A world position is useless without
the continent it belongs to, and `S_SPAWN_NPC` needs the creature's own `huntingZoneId` rather
than the one the player is standing in — pass the wrong zone and nothing appears. Looking the
name up gets both right at once.

The items table comes from the data center rather than the mod's TSV for the same reason. The
TSV has the localised name and nothing else, so item 9166 reads as "Recruit's Staff" — while
`ItemData` calls it `quest_collection_16`, category `quest`, `combatItemType="NO_COMBAT"`. It
merely carries a staff icon. Equipping it does nothing, and the name alone gives no way to tell.
Joining `ItemData` to `StrSheet_Item` keeps the readable name and adds the type, the required
class and the level, so `!items` shows what a thing actually is and `!equip` says so in red when
it is not equipment at all.

Note that `tera dc query` prints 20 rows by default; pass `--limit` before concluding a sheet is
short.

The distribution answers what the loadable continents actually are — 1, 2 and 3 carry the world
(202, 214 and 141 positions), 13 is the start zone, 7001 a side area. `ContinentData` is a
different thing: 20 entries holding channel configuration, which is why looking a zone up there
proves nothing. `CreateCharData/TutorialInitPos` points at continent 9015, for which no world
position exists at all, so the default spawn is `StartZone_ATW_P_x46y52_Center` on continent 13
instead.

Chat is HTML. The client wraps what you type in `<FONT>…</FONT>`, so the markup is stripped
before the command is read, and replies go back as `<FONT color="#F0FF89">…</FONT>` (`#EE1C24`
for errors, both taken from the client's own chat colour table). Replies go to the channel the
command came from.

Replies travel on **`S_DUNGEON_EVENT_MESSAGE`**, not `S_CHAT`: it carries free text, has a
`chat` flag that forces it into the chat window, and it is the one that renders here. `S_CHAT`
is encoded correctly — the offsets check out by hand — yet the client shows nothing, on any
channel, which points at the definition version rather than the payload. `!msgtest` is what
established that, by sending the same line through four packets at once; keep it around for the
next packet that goes quiet.

Equipment is stored per character, so it reaches the client three ways: `S_USER_EXTERNAL_CHANGE`
for an immediate visual change, `S_ITEMLIST` for the equipment window, and the item ids carried
by `S_LOGIN` and `S_GET_USER_LIST` so it survives a relog.

### Answering everything else

Two mechanisms cover the packets the server has no opinion about:

* `data/responses.json` maps a request name to the replies it should produce. Field values may
  be literals, `$gameId`/`$name`/`$level`/`$zone`/`$loc`/`$angle`/`$time`, or `$.field` to copy
  a field out of the request. Chat echo and the tip acknowledgement are written this way.
* `--auto-reply` answers any unhandled `C_X` with `S_X` when both an opcode and a definition
  exist, copying same-named fields from the request. That copy matters: the client repeats
  `C_SIMPLE_TIP_REPEAT_CHECK` forever if the reply comes back with the wrong `id`.
* `--auto-reply-aliases` additionally follows the request/reply naming conventions the opcode
  map reveals — `C_REQUEST_X` answers as `S_X`, `S_REQUEST_X`, `S_RESULT_X`, `S_RESPONSE_X` or
  `S_REPLY_X`; `C_RQ_X` as `S_RP_X`; `C_ASK_X` as `S_ANSWER_X`. It lifts coverage from 84
  client packets to 126, and it is **off by default because it crashed the client**: a
  zero-filled `S_GUILD_PERK_LIST` sent to a character with no guild took the session down.
  Breadth is not the goal — a generic reply that states something false is worse than silence.
  Packets worth answering properly belong in `responses.json`, written by hand.

Each auto-reply is logged the first time it is used, so the log doubles as a list of what the
client asked for and got a generic answer to.

### The GM tool

The client ships a working GM panel. Not leftover strings — `S1UI_InGameOperationTool.swf` is a
306 KB `GFxMovieInfo` carrying real ActionScript call sites (`RequestGod`, `RequestInvisible`,
`RequestNpcSpawn`, `KillMonster`, `Punishment`), registered in the data center's `LoadUI` table
alongside Inventory, and localised into English. Two more windows come with it: a raw command
console (`InGameManageTool`) and a bookmark teleporter (`InGameBookMark`) backed by
`BookmarkData`, 1,283 Krafton QA destinations still described in Korean.

It is bound to a key that no options screen shows, because it appears in none of the seven
`CustomizeKeyData` groups:

```
KeyMap           <KeyDefine name="AdminTool" command="AdminTool" devKey="true" when="down" />
KeyMapLocalized  <KeyDefine name="AdminTool" key1="Alt+A" />
```

`tera dc set` rebinds it in place without a full XML round trip — `--select
'/KeyMapLocalized/KeyDefine[@name="AdminTool"]' --set 'key1=Tilde'` moves it to the backtick.
Keep the original alongside; the rebuilt file keeps the same AES key, the same revision and an
identical sheet listing, so only that one attribute changes.

The commands themselves also shipped, in the `IngameManager` sheet: `itisagooddaytodie` and
`icanseedeadpeople` and `shutupplease` as toggles, `iamtheflashman`, `wakeupmywarrior`,
`iwanttomove`, `sticktogether`, `whereis` and `bookmark` as one-shots. They are **not** parsed
from the chat box — typing one sends an ordinary `C_CHAT` and nothing else happens — so they
belong to the console window.

What is still unsolved is the privilege gate. `S_ADMIN_PRIVILEGE` (35637) and
`S_QA_SET_ADMIN_LEVEL` (32223) are the two grant paths and neither has a published structure;
sending an `int32` level of 1 through 255 produces no reaction and no `C_ADMIN_*` in return.
Either the guessed structure is wrong or the retail build no longer honours `devKey`, and that
cannot be settled statically because `TERA.exe` is WinLicense-packed — its strings are encrypted,
not stripped, which a control search confirms (`S1Game`, `UObject`, `GFxMovie` all return zero).
41 of the 45 `C_ADMIN_*` / `S_ADMIN_*` opcodes have no definition either; the way to get them is
to open the panel and read what it emits.

### What the client asks the launcher

The client finds a window of class `LAUNCHER_CLASS` and sends it `WM_COPYDATA` messages whose
`dwData` is an event id, replying to the sender's `HWND`:

| in | out | payload |
| --- | --- | --- |
| 1 | 2 | account name, UTF-16 |
| 3 | 4 | session ticket, UTF-16 |
| 5 | 6 | server list, Protocol Buffers |

The server list is protobuf with UTF-16 text in `bytes` fields, and every field has to be
present — see `docs/formats.md`. The client answers event 1002 when it rejects the list and
1003 when it accepts it, which makes those two the fastest way to test a payload.

`tera-ipc-probe.exe` sends those three questions to every `LAUNCHER_CLASS` window it can find
and prints the answers, which is how the stub was verified without launching the game.
`tera-launcher.exe --probe` cycles through encoding variants, one per server-list request, so
pressing refresh in the client walks through them.

### Session crypto

Handshake: server sends four bytes (`01 00 00 00`), then client key 1 (128 bytes), server key 1,
client key 2, server key 2. Both sides then derive two keystreams by shifting and xoring the four
keys (−67, +29, −41 on modern clients), expanding each 128-byte key into 680 bytes through 34
passes of SHA-0, and seeding three shift registers of 55, 57 and 58 words that vote on which one
advances. Traffic after that is `[u16 size][u16 opcode][body]`, size including the header.

The keystream is stateful across unaligned writes: a 37-byte packet leaves the cipher owing four
minus one bytes to the next call. Getting that wrong desynchronises everything a packet later.
