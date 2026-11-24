# tera-guessmap

Recovers an opcode map (`NAME code`) from a raw packet capture, for a client
version that has no published map. The client stores opcodes only as numbers, so
names cannot be read from memory; this instead matches observed packets to the
definition files by structure.

## How it works

For every observed `(direction, opcode)` it keeps a few sample frames, then tests
each definition of the same direction: a definition matches when it both parses a
sample and re-encodes it to the exact same bytes (`read` then `write`, byte-equal).
An opcode that matches exactly one definition is assigned with confidence; the
remaining ties are resolved by the one-to-one constraint between opcodes and names
and are marked as guesses.

## Run

```
cargo run -p tera-guessmap --release -- <capture.jsonl> \
  --definitions data/definitions --patch 100 --out protocol.guessed.map
```

The capture is JSONL with `from` (`client`/`server`), `opcode`, and `hex` (the raw
body). `--score` compares the result against a `name` field if the capture carries
one. `--include-guesses` also writes the bijection-resolved guesses; by default only
the confident, unique matches are written.

## Scope

Precision on confident matches is high; coverage is bounded by how many opcodes the
capture exercises, so feed broad captures and merge several sessions. Structural
twins (two packets with the same layout) stay ambiguous and need a tie-break from
packet order or manual review.
