# octo-ledger — Disk-Backed Stores Plan

Makes the pipeline's memory footprint bounded so it can process transaction files larger than
primary memory. Today the engine holds **two** in-RAM maps that grow with the input:
`DepositStore` (`TxId → DepositRecord`, grows with `#deposits`) and `Ledger`
(`ClientId → Account`, grows with `#clients`). This plan puts each behind a trait with an
**in-memory** impl and a **Live** (disk-backed) impl, wires the Live path into production and
integration/scale tests, and keeps the in-memory path for lean unit tests.

Same house rules as the prior two plans (`octo-ledger-bootstrap-plan.md`,
`octo-ledger-dispute-resolve-chargeback-plan.md`): implement the ordered tasks one at a time,
verify after each, and don't deviate from the layering or the streaming constraint without
asking — both are load-bearing.

---

## 0. Execution protocol (how every step is run)

This is a hard requirement, applied **after each numbered task in §9**, not just at the end:

1. `cargo fmt` — code stays correctly formatted.
2. `cargo test` — the whole suite is **green** before moving to the next step. A step is not
   "done" until its tests pass.
3. `cargo clippy --all-targets -- -D warnings` — no new lints.
4. **Correctness review by a fresh subagent.** After each *meaningful* step (at minimum: the
   two Live-impl steps §9.4/§9.6, the engine rewrite §9.7, and a final pass), spawn a **new,
   clean-context subagent** to review the diff for correctness — encoding/decoding round-trip
   soundness, transaction/borrow handling, the guard order in the engine, and parity between
   the in-memory and Live impls. Use `/code-review` (or an equivalent clean-slate review
   agent). Feed its findings back before continuing. The point of a *fresh* agent is that it
   re-derives correctness from the code, not from this conversation's assumptions.

Keep the test harness **lean** (the repo's standing rule): one focused test per behavior, no
redundant coverage, no superfluous comments.
Your audience are senior sw devs, which don't need comments to understand code. Only add
comments if they're really needed.

---

## 1. Guiding principles

- **Streaming, not buffering — unchanged.** The transaction log is still consumed one record at
  a time; **never `.collect()` the entries into a `Vec`.** This plan only changes where the two
  *aggregate* structures live (RAM → disk), never how the input is read.
- **Bounded RAM is the goal.** After this change the production path holds in RAM only: one
  in-flight `Entry`, the small `Ledger` (see the u16 note below), and redb's bounded page cache.
  Every `DepositRecord` lives on disk.
- **Trait-first, owned values.** Both traits hand back **owned** values (`Option<Account>`,
  `Option<DepositRecord>`) rather than `&mut` references, because a disk-backed impl can only
  return owned data (it decodes bytes out of a read transaction). `DepositStore` already has
  this shape; `Ledger` gets refactored into the same shape.
- **Run-once, then die → no durability needed.** The tool runs once, writes stdout, and exits.
  We never re-open the data. So the Live store uses a **temporary** database (deleted on drop)
  and **`Durability::None`** — no fsync per commit. This is exactly the "not a full-blown
  database with persistence" intent: redb is used purely as a bounded-memory scratch structure,
  not as a store of record.
- **Strict layering unchanged.** `reader → parser → engine → printer → main`, each owning its
  error type. New code is confined to `store.rs`, a new `ledger.rs`, the `engine`/`printer`
  signatures, and `main`'s wiring. `reader`/`parser` are untouched.
- **`README.md` is the real docs** and must be brought in sync (§8) once the code lands.

---

## 2. Decisions locked in this planning round

Each has a matching test in §7.

1. **Disk backend = `redb`** (pure-Rust embedded B-tree, stable on-disk format, best
   point-read / single-writer perf, supports in-place mutation for `set_state`). This confirms
   the drop-in the prior plan and README already anticipated. `redb = "2"` is added as a normal
   dependency. `fjall` (LSM, write-heavy) and a hand-rolled spill file were considered and
   rejected: the workload is point-read + occasional in-place state flip, and hand-rolling a
   random-access, in-place-updatable index over a non-dense `u32` keyspace is high
   correctness-risk for no benefit. `sled` remains off the table (unstable on-disk format).

2. **The Live database is ephemeral, `Durability::None`.** It is created in a fresh temp
   directory (held by the store and removed on `Drop`), so a crash or normal exit leaves nothing
   behind. `Durability::None` drops per-commit fsync — safe here precisely because we never rely
   on the data surviving the process. This is what keeps millions of single-record inserts fast.

3. **Store trait methods stay infallible; only *construction* is fallible.** The requirements
   state we may "assume that secondary memory is fine (disk)." So `insert`/`get`/`set_state`
   keep their current infallible signatures, and a redb I/O error inside them is an
   unrecoverable environmental fault surfaced via `.expect(...)` with a clear message — it is
   **not** bad *input* (the README's "never panics" promise is about input, which still holds).
   What *can* fail cleanly is **opening** the database (temp-dir creation, redb open); that is a
   `Result` handled gracefully in `main` (nonzero exit, `error: … / caused by: …` chain, no
   stdout), before any row is processed. Rationale recorded so it's a conscious trade-off; if
   disk-failure resilience is ever wanted, the alternative is to make the trait methods
   `Result`-returning and thread `?` through the engine (a larger, deferred change).

4. **Both structures get a trait + in-memory impl; only `DepositStore` gets a Live impl now.**
   `ClientId` is a `u16`, so the `Ledger` is capped at **≤ 65,536 accounts** (~a few MB) and
   **cannot** exhaust memory. The real, unbounded OOM risk is `DepositStore` (`#deposits` is
   unbounded). So:
   - `DepositStore`: `InMemoryDepositStore` (unit tests) **+** `LiveDepositStore` (production,
     integration, scale). *This is the change that actually fixes the memory problem.*
   - `Ledger` → `LedgerStore` trait **+** `InMemoryLedger` only. The trait is introduced now
     (owned-value shape, ready for a `LiveLedger` redb drop-in) so the two structures are
     symmetric and swappable, but no disk ledger is built — it would be uniformity-theater for a
     structure that can't OOM. A one-paragraph note in the README explains why.

5. **Production path = Live deposits + in-memory ledger.** `main` builds `LiveDepositStore` +
   `InMemoryLedger`. Because the integration tests drive the compiled binary, they exercise the
   **Live** deposit store automatically (including the 50k-row scale test → real disk path at
   scale). Unit tests use the in-memory impls for speed.

---

## 3. Serialization format for `DepositRecord` (the Live value)

`DepositRecord` is fixed-size, which makes the on-disk encoding trivial and total:

| field    | type          | bytes | encoding                                             |
|----------|---------------|-------|------------------------------------------------------|
| `client` | `ClientId`(u16) | 2   | `u16::to_le_bytes` / `from_le_bytes`                 |
| `amount` | `Amount`(Decimal) | 16 | `rust_decimal::Decimal::serialize()` → `[u8; 16]`, `deserialize` back |
| `state`  | `DisputeState`  | 1   | `0 = NeverDisputed, 1 = Disputed, 2 = Settled`       |

Total **19 bytes**, fixed. Two private free functions in `store.rs`:

```rust
fn encode(record: &DepositRecord) -> [u8; 19];
fn decode(bytes: &[u8]) -> DepositRecord; // exhaustive; panics only on a corrupt/short slice,
                                          // which can't happen for our own writes
```

We don't need tests for these functions specifically, but make sure that at the end of the plan,
they are tested in some path.

`Decimal::serialize()` is a stable, lossless 16-byte representation (no precision loss for the
≤4-dp amounts), which is why it's preferred over string encoding. A unit test asserts
`decode(encode(r)) == r` for representative records, including 4-dp amounts and each state.

redb table type: `TableDefinition<u32, &[u8]>` (key = `TxId::into_inner()`, value = the 19-byte
slice). One table, e.g. `const DEPOSITS: TableDefinition<u32, &[u8]> = TableDefinition::new("deposits");`.

---

## 4. `store.rs` — add `LiveDepositStore` + `StoreError`

Keep the existing `DepositStore` trait and `InMemoryDepositStore` **unchanged**. Add:

```rust
pub struct LiveDepositStore {
    _dir: tempfile::TempDir,   // owns the temp dir; deleted on Drop
    db: redb::Database,
}

impl LiveDepositStore {
    /// Fallible: creates a temp dir + redb database with Durability::None.
    pub fn new() -> Result<Self, StoreError> { /* … */ }
}

impl DepositStore for LiveDepositStore {
    fn insert(&mut self, tx: TxId, record: DepositRecord) {
        // begin_write (Durability::None), open table, insert(tx, &encode(record)), commit
        // .expect on redb errors per decision 3
    }
    fn get(&self, tx: TxId) -> Option<DepositRecord> {
        // begin_read, open table, get(tx) -> map(|v| decode(v.value()))
    }
    fn set_state(&mut self, tx: TxId, state: DisputeState) {
        // begin_write: get current bytes -> decode -> set .state -> insert back -> commit
        // no-op if the tx isn't present (mirror InMemory behavior)
    }
}
```

`StoreError` (thiserror, owned by `store.rs`): variants wrapping `std::io::Error` (temp dir) and
`redb::DatabaseError` (open). Only construction uses it (decision 3).

Set `Durability::None` on every write transaction (or once at DB creation if the API allows).
`insert` is one write txn per deposit; that's the simple, correct shape, and `Durability::None`
keeps it fast enough. (If profiling later shows commit overhead dominates, an internal
write-batch is a possible future optimization — **not** in scope now; correctness first.)

**Dependencies (`Cargo.toml`):** add `redb = "2"` under `[dependencies]`; **move `tempfile`
from `[dev-dependencies]` to `[dependencies]`** (the Live store needs it on the production path).

---

## 5. `ledger.rs` (new) — `LedgerStore` trait + `InMemoryLedger`

Move the current `Ledger` out of `model.rs` into a new `ledger.rs`, refactored to the owned-value
trait shape. `Account`, `DepositRecord`, `Entry`, etc. stay in `model.rs`.

```rust
pub trait LedgerStore {
    fn get(&self, client: ClientId) -> Option<Account>;      // owned, disk-ready
    fn upsert(&mut self, client: ClientId, account: Account); // insert-or-replace
    /// Iterate all accounts for the printer. Owned values so a disk impl stays swappable.
    fn for_each_account(&self, f: impl FnMut(&Account));
}

#[derive(Debug, Default)]
pub struct InMemoryLedger {
    accounts: HashMap<ClientId, Account>,
}
// impl LedgerStore for InMemoryLedger { … }
```

Notes:
- `get` replaces `get_mut`; the engine now does **read → modify owned `Account` → `upsert`**
  instead of mutating in place. `Account` already derives `Clone`, so this is cheap and the u16
  bound makes it irrelevant at scale.
- **Get-or-create** stays in the *engine*, not the trait: deposit does
  `let mut acc = ledger.get(c).unwrap_or_else(|| Account::new(c));`. Withdrawal keeps its
  no-phantom-account rule by acting only when `get` returns `Some`.
- `for_each_account` (callback) rather than returning an iterator keeps the trait
  disk-compatible without associated-type/lifetime gymnastics, and the printer already writes
  row-by-row, so a callback fits.
- A `LiveLedger` (redb table `u16 → Account bytes`, range-scan for `for_each_account`) is a
  documented **future** drop-in, not built now (decision 4).

---

## 6. `engine.rs` + `printer.rs` — generalize over the ledger

Engine gains a ledger type parameter alongside the store, and returns the ledger it was given
(so `main`/tests can print it):

```rust
pub fn run_with_stores<D: DepositStore, L: LedgerStore>(
    entries: impl Iterator<Item = Entry>,
    mut deposits: D,
    mut ledger: L,
) -> L { /* … */ ledger }

// convenience for unit tests — both in-memory:
pub fn run(entries: impl Iterator<Item = Entry>) -> InMemoryLedger {
    run_with_stores(entries, InMemoryDepositStore::default(), InMemoryLedger::default())
}
```

The per-type logic and **guard order are unchanged** (lock → miss → client-match → state →
funds); only the mechanics of touching an account change: fetch owned `Account`, mutate, then
`ledger.upsert(client, account)`. Rework `validate_dispute_target` to return
`Option<(DepositRecord, Account)>` (owned account); the caller mutates and upserts. Preserve the
exact balance transitions (deposit `5/0/5` → dispute `0/5/5` → chargeback `0/0/0`+locked, etc.).

`printer::write_ledger` becomes generic: `write_ledger<L: LedgerStore, W: Write>(ledger: &L, w: W)`
and drives output via `ledger.for_each_account(|acc| writer.serialize(acc))` (thread the
`csv::Error` out — e.g. capture the first error in the closure, or use a small `try`-style
helper — keeping the existing "header written even with zero accounts" behavior).

---

## 7. `main.rs` — wire the Live path + graceful construction error

- `mod ledger;` added; `mod store;` already present.
- Build `LiveDepositStore::new()?` and `InMemoryLedger::default()`, pass both to
  `engine::run_with_stores(bridged, deposits, ledger)`.
- Surface `StoreError` through `main`'s error rendering. Minimal, layering-respecting wiring:
  add a `#[from] StoreError` bridge into the top-level error `main::run` returns (either widen it
  to a small `AppError` enum, or add a `ReaderError` variant — pick the smaller diff, keeping the
  `error: … / caused by: …` chain intact). Construction failure ⇒ nonzero exit, message on
  stderr, **no** stdout, before any processing (matches the existing malformed-input contract).
- Everything else in `main::run` (the lazy reader→engine bridge, first-error capture) is
  unchanged.

---

## 8. Tests

**Store contract, both impls (`store.rs`).** Keep the three existing `InMemoryDepositStore`
tests. Add a **generic contract helper** and run it against *both* impls so the Live impl is held
to the exact same behavior as the reference:

```rust
fn deposit_store_contract<S: DepositStore>(mut store: S) {
    // insert→get roundtrip returns the identical record (incl. a 4-dp amount);
    // set_state advances NeverDisputed→Disputed→Settled and get reflects it;
    // set_state on an absent tx is a no-op; get on an unknown tx is None.
}
#[test] fn in_memory_satisfies_contract() { deposit_store_contract(InMemoryDepositStore::default()); }
#[test] fn live_satisfies_contract()      { deposit_store_contract(LiveDepositStore::new().unwrap()); }
```

Plus **Live-specific correctness** unit tests:
- `encode`/`decode` round-trip for representative records (each `DisputeState`, a 4-dp amount).
- **Scale/persistence-across-ops:** insert several thousand distinct `tx`, read them all back
  correctly, `set_state` on a subset, verify — proving random-access get + in-place update hold
  once data has actually spilled through redb (not just a handful of rows).

**Ledger (`ledger.rs`).** Lean tests for `InMemoryLedger`: `upsert` then `get` roundtrips;
`get` on unknown client is `None`; `for_each_account` visits every upserted account exactly once.

**Engine (`engine.rs`).** Port all existing engine tests to the new `run`/`run_with_stores`
signatures (behavior identical). Add **one** engine test that runs a dispute→resolve and a
dispute→chargeback scenario through `run_with_stores(…, LiveDepositStore::new().unwrap(),
InMemoryLedger::default())`, asserting the same balances as the in-memory path — proving the
engine is correct against the disk store end-to-end, not just the store in isolation.

**Printer (`printer.rs`).** Update to the generic signature; existing header/empty-ledger/rows
tests otherwise unchanged.

**Integration (`tests/integration_test.rs`) + scale.** No new fixtures needed — the binary now
uses `LiveDepositStore`, so **all** integration tests (including the 50k-row scale test) already
exercise the Live disk path. Confirm they still pass. Keep the suite lean.

---

## 9. Ordered task list

Implement sequentially. Run the §0 protocol (fmt → test green → clippy → subagent review on
meaningful steps) after **each** one.

1. **`Cargo.toml`** — add `redb = "2"`; move `tempfile` to `[dependencies]`. `cargo build`.
2. **`store.rs`: encoding** — add `encode`/`decode` + the round-trip unit test. (No behavior
   change yet.)
3. **`store.rs`: `StoreError`** — add the thiserror enum (io + redb open).
4. **`store.rs`: `LiveDepositStore`** — temp-dir + redb DB (`Durability::None`), implement
   `DepositStore`. Add the generic `deposit_store_contract` helper, run it against both impls,
   and add the encode round-trip + few-thousand-record correctness tests. *(subagent review)*
5. **`ledger.rs` (new)** — move `Ledger` out of `model.rs`, refactor into `LedgerStore` trait +
   `InMemoryLedger` (owned `get`/`upsert`/`for_each_account`). Register `mod ledger;`. Add the
   lean ledger tests. Update `model.rs` (drop `Ledger`) and fix imports.
6. **`printer.rs`** — generalize `write_ledger` over `LedgerStore` via `for_each_account`; update
   its tests. *(subagent review — the error-threading in the callback is the subtle bit)*
7. **`engine.rs`** — `run_with_stores<D, L>` + `run` convenience; owned get/modify/upsert;
   `validate_dispute_target` returns owned `Account`. Port existing tests; add the one
   Live-store engine test. *(subagent review — guard order + balance transitions must be
   byte-for-byte the same)*
8. **`main.rs`** — `mod ledger;`, build `LiveDepositStore::new()?` + `InMemoryLedger`, call
   `run_with_stores`, thread `StoreError` into the graceful error path.
9. **Integration/scale** — run the full binary-driven suite; confirm the Live path is green at
   50k rows.
10. **`README.md`** — apply the §10 delta.
11. **Final pass** — `cargo fmt`, full `cargo test`, `cargo clippy`, and a **final fresh-subagent
    correctness review** of the whole diff. Remove any superfluous comments; confirm the test
    harness is lean (no redundant tests) and every new path is covered.

---

## 10. README delta (compare current → target)

Apply only after the code lands (updating earlier would misreport status):

- **Architecture → `store.rs`:** note it now ships both `InMemoryDepositStore` and the
  redb-backed `LiveDepositStore` (ephemeral temp DB, `Durability::None`), and add the new
  `ledger.rs` (the `LedgerStore` trait + `InMemoryLedger`).
- **Development / production path:** state that the binary uses `LiveDepositStore` +
  `InMemoryLedger`, so deposits live on disk and the integration/scale tests exercise the disk
  path.
- **Known limitations / future work:** rewrite the memory-model bullet. Deposits are **no longer
  retained in RAM** — the deposit index is disk-backed via redb, so RAM is now bounded to the
  streamed record, the small ledger, and redb's page cache. Add the honest **u16 note**: the
  ledger is intentionally in-memory because `ClientId: u16` caps it at ≤65,536 accounts
  (~a few MB); a `LiveLedger` is a trivial future redb drop-in behind the already-present
  `LedgerStore` trait, but is unnecessary for bounded memory today.
- Reread the whole README afterward for coherence — it is the project's real documentation.

Then do a recheck with a subagent that starts fresh (Sonnet), double check that the readme is
updated, makes sense and it's clean and not redundant, easy to read by a human, good documentation.

---

Ask if anything here is ambiguous before implementing.
