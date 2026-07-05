# octo-ledger — Dispute / Resolve / Chargeback Plan

Implements the remaining three transaction types on top of the bootstrap (`deposit`,
`withdrawal`). Same house rules as `octo-ledger-bootstrap-plan.md`: build the ordered tasks
one at a time, verify `cargo build` + `cargo test` after each, and ask before deviating —
the streaming constraint and the layering are still load-bearing.

---

## 1. Guiding principles (unchanged, plus one)

- **Streaming, not buffering.** The transaction log is still consumed one record at a time;
  **never `.collect()` the entries into a `Vec`.**
- **New nuance — a bounded deposit index.** Dispute/resolve/chargeback reference a *past
  deposit* by `tx` id and need its amount, its owning client, and its dispute state. The engine
  must therefore retain **every deposit** (never withdrawals — disputes apply only to deposits)
  keyed by `tx` id. This is a deliberate, documented change to the memory model: the tx log is
  still streamed, but engine memory now grows with the **number of distinct deposits**, i.e.
  `O(#clients + #deposits)`. See §4 for how we keep the door open to a truly-bounded impl.
- **Strict layering unchanged.** `reader → parser → engine → printer → main`, each owning its
  error type. The new logic is entirely inside `engine` + a new `store` module; no reader,
  parser, or error changes are required (all new event outcomes are *silent ignores*, never
  errors).
- **`README.md` is the real docs** and must be brought in sync (§9) once the code lands.

---

## 2. Decisions locked in this planning round

These were open questions; here is what we settled on and why. Every one has a matching test
in §8.

1. **Deposit lookup lives behind a `DepositStore` trait; ship the in-memory impl now.**
   Start against an interface so a disk-backed impl can be swapped in later without touching the
   engine. In-memory `HashMap<TxId, DepositRecord>` is the only impl we build now. Crate survey
   for the future disk impl (fixed-size key + fixed-size record makes this easy): **`redb`**
   (pure-Rust B-tree, stable format, best point-read/single-write perf, in-place mutation) is
   the recommended drop-in; `fjall` v3 (LSM, write-heavy) and `odht` (rustc's fixed-size on-disk
   hash table, but build-then-read oriented) are alternatives; **avoid `sled`** (still
   alpha/unstable on-disk format). No disk dependency is added in this round.

2. **A locked account freezes *everything*.** Once a chargeback locks a client, **all**
   subsequent events for that client are ignored — deposit, withdrawal, dispute, resolve, *and*
   chargeback. Simplest, safest invariant: a frozen account is fully inert. (Checked first, so a
   second outstanding dispute on a now-locked account cannot resolve or charge back.)

3. **A dispute is skipped when the client lacks the funds to hold.** If, at dispute time,
   `available < deposit.amount`, the dispute event is **dropped** and the deposit stays
   `NeverDisputed` (so it remains eligible for a future dispute). Consequence: `available`,
   `held`, and `total` are **never negative** anywhere in this engine — deposits only add,
   withdrawals apply only with sufficient `available`, disputes hold only with sufficient
   `available`. This is the notable non-default choice (the standard payments engine would allow
   `available` to go negative); we chose the funds-guarded variant deliberately.

4. **Per-deposit dispute state is a one-way 3-state machine:**
   `NeverDisputed → Disputed → Settled`. `Settled` is terminal and shared by both *resolve* and
   *chargeback* (we never need to distinguish them after the fact — a chargeback also locks the
   account, and a resolved/charged-back deposit can never be disputed again). This structurally
   guarantees "can't dispute twice" and "can't re-dispute a settled tx".

5. **Withdrawal now has an insufficient-funds guard.** A withdrawal with
   `available < amount` is ignored and no balances change (previously applied unconditionally —
   a bootstrap gap). A withdrawal for an unknown client (no account yet) has `available == 0`
   and is therefore ignored, materializing no phantom account.

6. **Client-mismatch on a referenced tx ⇒ ignore.** If a dispute/resolve/chargeback names a
   client different from the one that owns the referenced deposit, it's a partner-side error and
   the event is ignored.

7. **A dispute/resolve/chargeback referencing a nonexistent tx, or a *withdrawal's* tx, is
   ignored.** Because only deposits are indexed, a lookup miss covers both cases for free — a
   reference to a withdrawal is indistinguishable from a reference to a nonexistent tx, exactly
   as required.

8. **Big-file testing.** Add one committed ~500-row example (`large.csv`) with many clients and
   interleaved disputes for a readable end-to-end fixture, **plus** a programmatic test helper
   that writes a much larger temp CSV (tens of thousands of rows) at runtime to exercise scale
   without bloating the repo.

---

## 3. Data model changes (`model.rs` + new `store.rs`)

New types (kept small and `Copy` so the store API can hand back owned values cheaply):

```rust
// model.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    NeverDisputed,
    Disputed,
    Settled, // resolved or charged back — terminal, cannot be re-disputed
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepositRecord {
    pub client: ClientId, // for the client-mismatch guard (decision 6)
    pub amount: Amount,
    pub state: DisputeState,
}
```

`Account`, `Ledger`, `Entry`, `RawEntry`, `TxType` are unchanged. `Account` already carries
`held` and `locked`, which now actually vary.

New `store` module — the interface + the one impl:

```rust
// store.rs
pub trait DepositStore {
    /// Record a new deposit. tx ids are globally unique, so this never overwrites.
    fn insert(&mut self, tx: TxId, record: DepositRecord);
    /// Point lookup. Returns an owned record (works for in-memory and disk-backed).
    fn get(&self, tx: TxId) -> Option<DepositRecord>;
    /// Advance a deposit's dispute state in place.
    fn set_state(&mut self, tx: TxId, state: DisputeState);
}

#[derive(Default)]
pub struct InMemoryDepositStore {
    deposits: HashMap<TxId, DepositRecord>,
}
// impl DepositStore for InMemoryDepositStore { ... }
```

The owned-`get` + `set_state` shape (rather than `get_mut`) is what keeps a disk-backed impl
(redb et al., which return owned values) a straight swap.

---

## 4. Engine rewrite (`engine.rs`)

`run` keeps its public signature for `main`, but grows a store internally, and gains a generic
sibling for tests/future injection:

```rust
pub fn run(entries: impl Iterator<Item = Entry>) -> Ledger {
    run_with_store(entries, InMemoryDepositStore::default())
}

pub fn run_with_store<S: DepositStore>(entries: impl Iterator<Item = Entry>, mut store: S) -> Ledger {
    let mut ledger = Ledger::new();
    for entry in entries {
        // ... per-type handling below ...
    }
    ledger
}
```

Remove the old `// TODO` streaming-deferral comment (this is now the conscious decision it
flagged). Per-type handling — **the lock check comes first for every type** (decision 2):

- **Deposit** — if the account exists and is `locked`, ignore. Otherwise get-or-create the
  account, `available += amount`, `total += amount`, and `store.insert(tx, {client, amount,
  NeverDisputed})`.
- **Withdrawal** — if the account exists and is `locked`, ignore. Look up the account; if it
  doesn't exist or `available < amount`, ignore (no phantom account). Otherwise `available -=
  amount`, `total -= amount`. (Withdrawals are **not** stored — never disputable.)
- **Dispute** — `store.get(tx)`; ignore on miss (covers nonexistent + withdrawal refs,
  decision 7). Ignore if `record.client != entry.client` (decision 6). Ignore if the account is
  `locked` (decision 2). Ignore if `record.state != NeverDisputed` (can't dispute twice / a
  settled tx, decision 4). Ignore if `account.available < record.amount` (decision 3).
  Otherwise `available -= amount`, `held += amount` (total unchanged), and
  `store.set_state(tx, Disputed)`.
- **Resolve** — `store.get(tx)`; ignore on miss / client-mismatch / locked. Ignore if
  `record.state != Disputed` (**the not-under-dispute edge case**). Otherwise `held -= amount`,
  `available += amount` (total unchanged), and `store.set_state(tx, Settled)`.
- **Chargeback** — same guards as resolve (miss / mismatch / locked / not `Disputed`).
  Otherwise `held -= amount`, `total -= amount` (available unchanged), `store.set_state(tx,
  Settled)`, and set `account.locked = true`.

Because a dispute only fires when `available >= amount`, `held` for any disputed tx is always
`>= amount` at resolve/chargeback time, so those subtractions never go negative either.

Reference trace (deposit → dispute → chargeback): `5/0/5` → `0/5/5` → `0/0/0` + locked. Clean
reversal.

---

## 5. Reader / parser / error — no changes

All new event outcomes are silent ignores, not failures, so `ParseError`/`ReaderError` are
untouched. The parser already drops any stray `amount` on dispute/resolve/chargeback rows and
enforces amount-presence on deposit/withdrawal. Confirm the existing tests still pass; add
nothing here.

---

## 6. Printer — no code changes, wider value range

`held`, `total`, and `locked` now take non-trivial values, but `write_ledger` already
serializes the whole `Account`. The existing header/empty-ledger tests still hold. (Values stay
non-negative per decision 3, so no formatting surprises.)

---

## 7. Example files (`examples/`)

Keep `happy_path.csv` and `malformed.csv`. Add small, human-readable scenario fixtures plus one
larger one:

- `dispute.csv` — deposit then dispute; asserts funds move available→held, total unchanged.
- `resolve.csv` — deposit, dispute, resolve; back to all-available.
- `chargeback.csv` — deposit, dispute, chargeback; `held`/`total` drop, `locked = true`.
- `edge_cases.csv` — one file threading the ignore-paths: client-mismatch dispute,
  dispute-twice, resolve-when-not-disputed, dispute-on-a-withdrawal-tx, an event after lock,
  and a funds-short dispute (decision 3).
- `large.csv` — ~500 rows, many clients, interleaved deposits/withdrawals/disputes/resolves/
  chargebacks; a readable end-to-end scale fixture.

---

## 8. Tests

**Unit (`engine.rs`, `store.rs`)** — one test per decision in §2, at minimum:
- dispute holds funds (available↓, held↑, total flat); resolve reverses it; chargeback drops
  held+total and locks.
- dispute on nonexistent tx → ignored.
- dispute/resolve/chargeback on a **withdrawal's** tx → ignored (treated as nonexistent).
- **client-mismatch** on dispute/resolve/chargeback → ignored.
- **dispute twice** → second ignored.
- **resolve when not under dispute** → ignored.
- **resolved tx cannot be disputed again** (Settled is terminal) → ignored.
- chargeback locks; then deposit / withdrawal / dispute / resolve / chargeback on the locked
  account are **all** ignored (decision 2).
- **funds-short dispute** ignored, deposit stays disputable (decision 3).
- withdrawal insufficient funds → ignored, balances unchanged; withdrawal on unknown client →
  no account created.
- `store`: insert/get roundtrip, `set_state` advances, get-miss is `None`.

**Integration (`tests/integration_test.rs`)** — drive the binary against each new example and
assert the rendered rows. Keep the existing happy-path + malformed tests.

**Scale test** — a helper that writes a large temp CSV (e.g. 50k rows across many clients with
interleaved disputes), runs the binary, and asserts the per-row invariant `total == available +
held` on every output row (plus a couple of spot-checked balances). Clean up the temp file.

---

## 9. README delta (compare current → target)

The current README is accurate for the bootstrap but now stale in these spots — update as part
of the final task (do **not** update before the code lands; it would misreport status):

- **"Current bootstrap status" note** (dispute/resolve/chargeback are no-ops, `held` always 0,
  `locked` always false): **delete** — they now work.
- **Transaction-type bullets**: expand `dispute`/`resolve`/`chargeback` with the real
  semantics from §4, including "references a prior *deposit*; a reference to a
  withdrawal/nonexistent/other-client tx is ignored".
- **Behavioral rules to add** (previously unstated): locked freezes everything (decision 2);
  dispute is skipped when `available < amount` and never drives balances negative (decision 3);
  withdrawal insufficient-funds guard (decision 5).
- **Architecture section**: add `store.rs` (the `DepositStore` trait + `InMemoryDepositStore`)
  and note the engine now holds a bounded deposit index.
- **Known limitations / future work**: remove the two now-resolved bullets
  (dispute-as-no-op, no-withdrawal-guard). Replace with an honest **memory-model note**: engine
  memory is `O(#clients + #deposits)`; the tx log is still streamed; a truly-bounded run is a
  future swap of `DepositStore` for a disk-backed impl (`redb` recommended), for which the trait
  is already in place.

As a final check, reread the readme, ensure everything makes sense, the readme is very important as it is our docs.

---

## 10. Ordered task list

Implement sequentially; `cargo build` + `cargo test` after each meaningful step.

1. **`model.rs`** — add `DisputeState` enum and `DepositRecord` struct (§3).
2. **`store.rs`** — new module: `DepositStore` trait + `InMemoryDepositStore`; register `mod
   store;` in `main.rs`. Unit-test insert/get/set_state.
3. **`engine.rs`** — rewrite `run` → `run_with_store` per §4: deposit stores records; withdrawal
   gains the funds guard; dispute/resolve/chargeback implement the state machine with the guard
   order (lock → miss → client-match → state → funds). Remove the old TODO. Port existing engine
   tests; add the §8 unit tests.
4. **`examples/`** — add `dispute.csv`, `resolve.csv`, `chargeback.csv`, `edge_cases.csv`,
   `large.csv` (§7).
5. **`tests/integration_test.rs`** — assertions for each new example + the programmatic scale
   test (§8).
6. **`README.md`** — apply the §9 delta.
7. Final cargo.fmt check. Ensure we didn't write any superfluous comment in any file, ensure the tests are lean and we don't have too many of them, ensure everything is tested, ensure all tests are passing.

Ask if anything here is ambiguous before implementing.
