# octo-ledger

A small command-line tool that replays a stream of client transactions and
produces a per-client account ledger.

## Usage

```sh
cargo run -- transactions.csv > accounts.csv
```

The tool reads the transactions CSV given as its single argument, processes
it record-by-record without ever buffering the whole file into memory, and
writes the resulting ledger as CSV to stdout. Errors (a missing file, a
malformed row, etc.) are reported on stderr and the process exits with a
nonzero status; the tool never panics on bad input.

## Input format

The input is a CSV file with a header row:

```
type,client,tx,amount
```

| Column   | Type                  | Notes                                    |
|----------|-----------------------|-------------------------------------------|
| `type`   | one of the tx types below (lowercase) | see below |
| `client` | `u16`                 | client account id                          |
| `tx`     | `u32`                 | transaction id                             |
| `amount` | decimal                | present only for some tx types (see below) |

Transaction types:

- `deposit` — adds funds to a client's account. **Requires** `amount`.
- `withdrawal` — removes funds from a client's account. **Requires** `amount`.
- `dispute` — flags a prior transaction as disputed. **No** `amount` column.
- `resolve` — resolves a prior dispute. **No** `amount` column.
- `chargeback` — reverses a disputed transaction. **No** `amount` column.

A `deposit`/`withdrawal` row without an `amount` is a validation error and
causes the whole run to fail gracefully (nonzero exit, error printed to
stderr, no output on stdout). An `amount` present on a `dispute`/`resolve`/
`chargeback` row is silently ignored, since those transaction types don't
carry a value of their own.

## Output format

The output is a CSV with one row per client:

```
client,available,held,total,locked
```

- `available` — funds available for withdrawal.
- `held` — funds currently held due to a dispute.
- `total` — `available + held` (this invariant always holds).
- `locked` — `true` once a chargeback has occurred on the account, `false`
  otherwise.

**Current bootstrap status:** `dispute`, `resolve`, and `chargeback` rows are
parsed but currently processed as no-ops (see [Limitations](#known-limitations--future-work)
below), so `held` stays at `0` and `locked` is always `false` in this version.

## Architecture

Processing is split into a small pipeline of independently-testable layers,
each living in its own file under `src/`:

- **`reader.rs`** — streams the input file with `csv::Reader`, deserializing
  one record at a time and tracking row numbers for error messages.
- **`parser.rs`** — pure validation: converts a raw, untrusted CSV row into
  the validated `Entry` domain type (enforces the amount-presence rule).
- **`engine.rs`** — lazily folds the stream of validated `Entry` values into
  a `Ledger` (a per-client map of running balances).
- **`printer.rs`** — serializes the final `Ledger` to CSV.
- **`main.rs`** — the CLI entry point (via `clap`); wires the above stages
  together, bridging the reader's fallible per-row stream into the engine's
  plain `Entry` stream without collecting into a `Vec`, and prints error
  chains (`error: ...` / `  caused by: ...`) on failure.

Validation errors (`ParseError`) and I/O/CSV errors (`ReaderError`) are kept
in separate types (`src/error.rs`), both via `thiserror`, so the failure
reason stays precise and each error's `#[source]` chain prints cleanly.

## Development

```sh
cargo build   # compile
cargo run -- <path-to-csv>   # run against a transactions file
cargo test    # unit tests (reader, parser, engine, printer) + integration tests
```

Integration tests (`tests/integration_test.rs`) drive the compiled binary as
a subprocess against the fixtures in `examples/` (`happy_path.csv` and
`malformed.csv`), since this crate has no `lib.rs` and its internal modules
aren't reachable from outside the binary.

## Known limitations / future work

- **Dispute / resolve / chargeback are no-ops.** Implementing them properly
  requires looking up the amount of a *past* transaction by its `tx` id,
  which means retaining prior transactions (or at least prior deposits) in
  memory keyed by id — that reintroduces an O(all-transactions) memory
  footprint that the current streaming fold is designed to avoid. This is a
  deliberate deferral (see the `TODO` in `engine.rs`), not an oversight.
- **No insufficient-funds guard on withdrawals.** A withdrawal is currently
  applied unconditionally, even if it would drive `available` negative.
