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
- `withdrawal` — removes funds from a client's account, if there's enough
  `available` to cover it. **Requires** `amount`.
- `dispute` — places a hold on a prior *deposit*'s funds: moves `amount` from
  `available` to `held` (`total` unchanged), but only if `available` currently
  covers it. **No** `amount` column.
- `resolve` — releases the hold placed by a `dispute`, moving `amount` back
  from `held` to `available` (`total` unchanged). **No** `amount` column.
- `chargeback` — reverses a disputed deposit permanently: drops both `held`
  and `total` by `amount`, and **locks** the account. **No** `amount` column.

`dispute`/`resolve`/`chargeback` all reference a prior transaction by its
`tx` id, and only a *deposit*'s `tx` id is trackable — a reference to a
nonexistent `tx`, to a `withdrawal`'s `tx`, or to a deposit owned by a
different client is silently ignored (not an error).

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

## Behavioral rules

- **A locked account ignores everything.** Once a chargeback locks a client's
  account, every subsequent event for that client — deposit, withdrawal,
  dispute, resolve, or chargeback alike — is silently ignored.
- **A deposit can be disputed at most once, ever.** Its dispute state moves
  `NeverDisputed -> Disputed -> Settled` (settled via either `resolve` or
  `chargeback`); `Settled` is terminal, so a deposit that's already been
  resolved or charged back can never be disputed again.
- **A dispute is skipped if funds are short.** If `available` doesn't cover
  the disputed deposit's `amount`, the `dispute` is dropped rather than
  applied — the deposit stays `NeverDisputed` and can still be disputed
  successfully later. Combined with the withdrawal guard below, this means
  `available`, `held`, and `total` never go negative anywhere in the engine.
  This is a deliberate, non-default interpretation of the spec (the more
  common choice is to let a dispute drive `available` negative).
- **A withdrawal is skipped, not erroring, if it can't be satisfied** — either
  because the client has no account yet or because `available` is
  insufficient. No phantom accounts are created for unknown clients.

## Architecture

Processing is split into a small pipeline of independently-testable layers,
each living in its own file under `src/`:

- **`reader.rs`** — streams the input file with `csv::Reader`, deserializing
  one record at a time and tracking row numbers for error messages.
- **`parser.rs`** — pure validation: converts a raw, untrusted CSV row into
  the validated `Entry` domain type (enforces the amount-presence rule).
- **`store.rs`** — the `DepositStore` trait plus its `InMemoryDepositStore`
  implementation: an in-memory lookup of prior deposits by `tx` id, keyed by
  `client`/`amount`/dispute-state, used by `engine.rs` to process
  `dispute`/`resolve`/`chargeback` rows.
- **`engine.rs`** — lazily folds the stream of validated `Entry` values into
  a `Ledger` (a per-client map of running balances), consulting a
  `DepositStore` to resolve dispute/resolve/chargeback rows against the
  deposit they reference.
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
cargo test    # unit tests (reader, parser, store, engine, printer) + integration tests
```

Integration tests (`tests/integration_test.rs`) drive the compiled binary as
a subprocess against the fixtures in `examples/` (`happy_path.csv`,
`malformed.csv`, `dispute.csv`, `resolve.csv`, `chargeback.csv`, and
`edge_cases.csv`), since this crate has no `lib.rs` and its internal modules
aren't reachable from outside the binary. `examples/large.csv` is a
human-readable sample only, distinct from the ~50k-row CSV that
`cargo test`'s scale test generates on the fly to exercise the streaming
path at scale.

## Known limitations / future work

- **Memory footprint is O(#clients + #deposits), not O(#clients).** The
  engine still never buffers the transaction log itself into a `Vec` — it
  folds the input stream one row at a time — but it now retains every
  *deposit* record (not withdrawals) for the lifetime of the run, since
  `dispute`/`resolve`/`chargeback` fundamentally require looking up a past
  deposit's amount and dispute state by `tx` id. This is a deliberate
  trade-off, not an oversight.
- **`DepositStore` is a trait for exactly this reason.** `InMemoryDepositStore`
  is the only implementation today, but the trait boundary in `store.rs`
  exists so a disk-backed implementation (e.g. `redb`, a pure-Rust embedded
  B-tree) could be swapped in later — without changing `engine.rs` — to get
  a truly bounded-memory pipeline. No disk-backed implementation exists yet.
