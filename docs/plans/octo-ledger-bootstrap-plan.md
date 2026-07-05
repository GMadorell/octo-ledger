# octo-ledger — Bootstrap Plan

A Rust CLI that ingests a transactions CSV and emits per-client account balances.

```
$ cargo run -- transactions.csv > accounts.csv
```

This document is the implementation brief for the agent. Build it in the ordered tasks
below, one at a time, verifying each compiles and passes before moving on. Ask before
deviating from the architecture; the layering and the streaming constraints are load-bearing.

---

## 1. Guiding principles (read first, they constrain everything)

- **Streaming, not buffering.** Input files may be larger than available memory. Transactions
  are read and processed one record at a time. **Never `.collect()` the transactions into a
  `Vec`.** Memory must stay bounded to ~one record plus the aggregated ledger.
- **Strict layering.** Each layer has one job and owns its own error type. Lower-layer errors
  are wrapped as `#[source]` of higher-layer errors.
  - `reader` — I/O + CSV tokenizing. Owns the `csv::Reader`. Produces raw records.
  - `parser` — pure `raw record -> domain model`. **No I/O, no CSV, no file awareness.**
  - `engine` — folds a lazy stream of entries into a `Ledger`. Close to a noop for now.
  - `printer` — turns the typed `Ledger` into CSV output at the edge.
  - `main` — the controller: parses CLI args, wires the above together, owns
    human-readable error presentation and exit codes.
- **Newtypes everywhere** for parsed values, via [`nutype`](https://github.com/greyblake/nutype).
- **Typed errors throughout the library (`thiserror`); no `anyhow`.** `main` is the single
  boundary that renders errors for humans. Avoid one giant crate-wide error enum — per-layer
  types age better.
- **`Claude.md`** is a thin map: project structure and little else. **`README.md`** is the real
  human-readable project documentation and is kept up to date. Note in `Claude.md` that the
  README serves as the project documentation.

---

## 2. Data model overview

There are **two** model layers — do not conflate them:

1. **`Entry`** — one parsed input row (an instruction: deposit / withdrawal / dispute /
   resolve / chargeback, referencing a client and tx, optionally with an amount).
2. **`Account`** (or `AccountSummary`) — one output row: `client, available, held, total, locked`.

The **`Ledger`** is the engine's aggregate state: a map from client id to `Account`. It is
bounded by the number of distinct clients (`u16` ⇒ ≤ 65,536 small structs), independent of
transaction count.

### Field/type decisions

- **`amount` uses `rust_decimal::Decimal`, never `f64`.** The spec pins 4 decimal places of
  precision on input and output; floats will drift on summation. Wrap `Decimal` in a nutype.
- **Transaction `type` is an `enum`** (`Deposit`, `Withdrawal`, `Dispute`, `Resolve`,
  `Chargeback`), not a string newtype. Unknown types become a parse error for free, and the
  eventual `match` in the engine is exhaustive. Deserialize it via serde (`rename_all`).
- `client` is a `u16` newtype, `tx` is a `u32` newtype.
- `amount` is present for deposit/withdrawal and absent for dispute/resolve/chargeback; model
  it as `Option<Amount>` on the raw record and validate the presence rule in the parser.

---

## 3. Project structure

```
octo-ledger/
├── Cargo.toml
├── Cargo.lock
├── Claude.md               # thin map: structure only
├── README.md               # real human-readable project docs
├── examples/
│   ├── happy_path.csv       # well-formed, exercises the output shape
│   └── malformed.csv        # invalid CSV / bad row, to test graceful failure
├── src/
│   ├── main.rs              # controller: CLI -> reader -> engine -> printer; error presentation
│   ├── reader.rs            # csv::Reader, streaming, raw records
│   ├── parser.rs            # RawEntry -> Entry, pure, no I/O
│   ├── engine.rs            # folds Iterator<Item = Entry> into a Ledger
│   ├── printer.rs           # Ledger -> CSV output
│   ├── model.rs             # newtypes, TxType enum, Entry, Account, Ledger
│   └── error.rs             # per-layer error enums
└── tests/
    └── integration_test.rs  # near-end-to-end, drives the engine by example-file path
```

---

## 4. Dependencies (`Cargo.toml`)

- `csv` — CSV parsing (streaming).
- `serde` (with `derive`) — (de)serialization.
- `rust_decimal` (with the `serde` feature) — decimal amounts.
- `nutype` (with the `serde` feature) — newtypes with validation.
- `clap` (with `derive`) — CLI argument parsing.
- `thiserror` — typed error enums.

Pin exact versions in `Cargo.lock` (commit it). Gitignore is already handled.

---

## 5. CSV parsing: streaming with the `csv` crate

The `csv` crate reads record-by-record and reuses an internal buffer, so it streams with
bounded memory **provided we consume the iterator lazily**. Key implementation notes for the
agent:

- Use the **owning** iterator `into_deserialize::<RawEntry>()` (or `into_records()`), **not**
  the borrowing `deserialize()`. The borrowing form ties the iterator to a `&mut` on a local
  reader that drops at function end and will not compile when returned.
- Return **`Result<impl Iterator<Item = Result<Entry, ReaderError>>, ReaderError>`**, not a
  bare `impl Iterator`. Opening the file can fail — fail fast on open via the outer `Result`;
  per-record failures ride inside each `Item`.
- `csv::Reader` buffers internally — **do not** wrap the file in a `BufReader` yourself.
- Enable flexible whitespace handling as needed (spacing in the sample data is inconsistent;
  set `.trim(csv::Trim::All)` on a `ReaderBuilder`).
- Attach a **row number** to parse errors by reading the reader's position, so the
  human-readable message can name the offending row.

Reference shape (illustrative — the agent should adapt error variants to the final `error.rs`):

```rust
pub fn read_entries(
    path: &Path,
) -> Result<impl Iterator<Item = Result<Entry, ReaderError>>, ReaderError> {
    let rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|source| ReaderError::Open { path: path.to_path_buf(), source })?;

    let iter = rdr.into_deserialize::<RawEntry>().map(|res| {
        res.map_err(ReaderError::Csv)                     // csv::Error -> ReaderError
            .and_then(|raw| {
                Entry::try_from(raw)                      // parser: raw -> model
                    .map_err(|source| ReaderError::Parse { row: /* from position */ 0, source })
            })
    });

    Ok(iter)
}
```

The parser is the pure half:

```rust
// parser.rs — no I/O, no CSV awareness
impl TryFrom<RawEntry> for Entry {
    type Error = ParseError;
    fn try_from(raw: RawEntry) -> Result<Self, Self::Error> {
        // newtype construction + validation (amount presence rules, etc.)
    }
}
```

---

## 6. Error modeling (thiserror, per-layer, `#[source]` chaining)

Per-layer enums in `error.rs`. Lower errors are the `#[source]` of higher ones. Use `#[from]`
**only** where the conversion is unambiguous and needs no extra context; construct explicitly
where you want to attach context like a row number.

```rust
use std::path::PathBuf;
use thiserror::Error;

// Layer 1 — pure raw -> model. No I/O, no CSV.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid amount {0:?}")]
    Amount(String),
    #[error("unknown transaction type {0:?}")]
    TransactionType(String),
    #[error("missing amount for {0:?} transaction")]
    MissingAmount(String),
    // nutype generates a validation error per newtype — wrap transparently:
    // #[error(transparent)]
    // Client(#[from] ClientIdError),
}

// Layer 2 — I/O + CSV, wraps ParseError.
#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("could not open input file {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },
    #[error("malformed CSV")]
    Csv(#[source] csv::Error),
    #[error("could not parse row {row}")]
    Parse {
        row: u64,
        #[source]
        source: ParseError,
    },
}
```

Principles to hold to:
- Keep the wrapped error on `#[source]`, **not** folded into the display string — that is what
  lets `main` print a proper `caused by:` chain.
- Use `#[error(transparent)]` to pass through nutype's generated validation messages.
- Do **not** derive `From<ParseError> for ReaderError` — you want the row context, so build the
  `Parse` variant explicitly in the reader's `.map()`.

### `main.rs` — explicit human-readable presentation (Approach A)

No extra deps; full control over stderr wording and exit code.

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                eprintln!("  caused by: {s}");
                src = s.source();
            }
            ExitCode::FAILURE
        }
    }
}

// run() parses CLI args (clap), calls reader::read_entries, feeds the lazy
// iterator into engine, gets a Ledger, hands it to printer. Returns a top-level
// error type (or ReaderError for now).
```

Failure requirements: missing input arg, nonexistent file, and invalid CSV must all fail
**gracefully** with a human-readable message and a nonzero exit code — never a panic/backtrace.

---

## 7. Engine (noop-ish for now, but streaming-correct)

- Signature consumes a **lazy** stream: `engine::run(entries: impl Iterator<Item = Entry>) -> Ledger`
  (threading `Result` through as appropriate). It must fold entries as they arrive and **must
  not** `.collect()` upstream.
- For the bootstrap, applying deposits (and summing into `available`/`total`) is enough to make
  the example output coherent. Dispute/resolve/chargeback logic comes later.
- **Design note to carry forward (not to implement now):** dispute/resolve/chargeback reference
  a *past* `tx` id and its amount. The naive future implementation keeps every past transaction
  in memory, silently reintroducing the O(all-transactions) footprint we are avoiding. Leave a
  `// TODO` at the fold site flagging this so the streaming property is a conscious future
  decision, not an accident.

Output invariants the printer/engine must satisfy: `available = total - held`,
`held = total - available`, `total = available + held`, `locked = true` iff a chargeback
occurred. Up to 4 decimal places, in and out. Column spacing does not matter.

---

## 8. Example files (`examples/`)

- `happy_path.csv` — well-formed rows (deposits across ≥2 clients) that produce output matching
  the shape below.
- `malformed.csv` — an invalid CSV / bad row to exercise graceful failure.

Expected happy-path output shape (spacing irrelevant, 4dp max):

```
client, available, held, total, locked
1, 1.5, 0.0, 1.5, false
2, 2.0, 0.0, 2.0, false
```

---

## 9. Integration tests (`tests/integration_test.rs`)

- The engine is reachable as a function taking an already-parsed **path**, so tests can point it
  at the example files and assert on the resulting `Ledger` / rendered output.
- Cover: happy path produces the expected per-client balances; malformed input yields the
  expected typed error (not a panic).
- Keep them near-end-to-end — cross several layers, as close to end-to-end as makes sense.

---

## 10. Ordered task list for the agent

Implement sequentially; verify `cargo build` + `cargo test` after each where meaningful.

1. **Init repo & `Cargo.toml`.** Add the dependencies from §4. Confirm `cargo build` on an empty
   `main`. Commit `Cargo.lock`.
2. **`model.rs` — newtypes & enums.** `ClientId(u16)`, `TxId(u32)`, `Amount(Decimal)` via nutype;
   `TxType` enum; `RawEntry` (serde-deserializable, `amount: Option<...>`); `Entry`; `Account`;
   `Ledger`.
3. **`error.rs`.** `ParseError` and `ReaderError` per §6.
4. **`parser.rs`.** `TryFrom<RawEntry> for Entry` — pure, validates amount-presence rules and
   constructs newtypes. Unit-test it directly.
5. **`reader.rs`.** Streaming `read_entries` per §5 (owning iterator, outer `Result`, `Trim::All`,
   row numbers on parse errors).
6. **`engine.rs`.** Lazy fold into `Ledger`; deposits applied; `// TODO` streaming note per §7.
7. **`printer.rs`.** `Ledger` -> CSV via serde/csv writer, 4dp.
8. **`main.rs`.** clap CLI, wire reader -> engine -> printer, error presentation per §6
   (Approach A), graceful failures + exit codes.
9. **`examples/`.** Add `happy_path.csv` and `malformed.csv`.
10. **`tests/integration_test.rs`.** Drive by example-file path; assert happy path and the
    malformed-input error.
11. **Docs.** `Claude.md` (thin map) and `README.md` (real docs); note the README-as-docs
    convention in `Claude.md`.

Ask if anything here is ambiguous before implementing.
