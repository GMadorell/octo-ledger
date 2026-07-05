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

## Development

```sh
cargo build   # compile
cargo run -- <path-to-csv>   # run against a transactions file
cargo test    # unit tests (reader, parser, store, engine, printer) + integration tests
```

Integration tests (`tests/integration_test.rs`) drive the compiled binary as
a subprocess against the fixtures in `examples/`, since this crate has no `lib.rs` and its internal modules
aren't reachable from outside the binary.

The compiled binary always uses `LiveDepositStore` + `InMemoryLedger` in
production, so integration tests (including the scale test), which drive
that binary, exercise the real disk-backed deposit path end-to-end.
Unit tests, by contrast, mostly use the in-memory `InMemoryDepositStore` for
speed and to keep the store swappable behind its trait — `store.rs` also has
tests that run the same shared contract against `LiveDepositStore` directly.

## Primary vs secondary memory considerations

- **Deposits are retained on disk, not in memory.** `dispute`/`resolve`/
  `chargeback` require looking up a deposit's amount and dispute state by
  `tx` id, so *some* form of retention is required — that retention lives in
  `LiveDepositStore` (see the `store.rs` bullet under Architecture).
- **The per-client ledger stays in memory, on purpose** (see the `ledger.rs`
  bullet under Architecture for the `u16` bound that makes this safe). A
  disk-backed `LiveLedger` would add uniformity, not a real memory-safety
  improvement, so none exists; the `LedgerStore` trait is shaped so one could
  be added as a drop-in without touching `engine.rs`.
- **What's actually in memory during a run:** one in-flight `Entry` at a time
  (the input is streamed, never buffered into a `Vec`), the small in-memory
  ledger described above, and whatever page cache `redb` itself keeps for the
  deposit database. That's a bounded, modest footprint that doesn't grow with
  the number of deposits — not literally zero memory usage, but bounded.
