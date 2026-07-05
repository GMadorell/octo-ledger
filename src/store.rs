use crate::model::{Amount, ClientId, DepositRecord, DisputeState, TxId};
use redb::ReadableTable;
use rust_decimal::Decimal;
use thiserror::Error;

pub trait DepositStore {
    fn insert(&mut self, tx: TxId, record: DepositRecord);
    fn get(&self, tx: TxId) -> Option<DepositRecord>;
    fn set_state(&mut self, tx: TxId, state: DisputeState);
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct InMemoryDepositStore {
    records: std::collections::HashMap<TxId, DepositRecord>,
}

#[cfg(test)]
impl DepositStore for InMemoryDepositStore {
    fn insert(&mut self, tx: TxId, record: DepositRecord) {
        self.records.insert(tx, record);
    }

    fn get(&self, tx: TxId) -> Option<DepositRecord> {
        self.records.get(&tx).copied()
    }

    fn set_state(&mut self, tx: TxId, state: DisputeState) {
        if let Some(record) = self.records.get_mut(&tx) {
            record.state = state;
        }
    }
}

const DEPOSITS: redb::TableDefinition<u32, &[u8]> = redb::TableDefinition::new("deposits");

pub struct LiveDepositStore {
    // Field order matters: Rust drops fields in declaration order, and the
    // `redb::Database` handle must close before its backing temp dir is
    // removed.
    db: redb::Database,
    _dir: tempfile::TempDir,
}

#[derive(Debug, Error)]
pub enum LiveDepositStoreError {
    #[error("failed to create temporary directory for deposit store")]
    TempDir(#[source] std::io::Error),
    #[error("failed to open deposit store database")]
    OpenDatabase(#[source] redb::DatabaseError),
}

impl LiveDepositStore {
    pub fn new() -> Result<Self, LiveDepositStoreError> {
        let dir = tempfile::TempDir::new().map_err(LiveDepositStoreError::TempDir)?;
        let db_path = dir.path().join("deposits.redb");
        let db = redb::Database::create(db_path).map_err(LiveDepositStoreError::OpenDatabase)?;

        // Pre-create the table so `get` (a read transaction) never sees a
        // missing table before the first `insert`/`set_state` (a write
        // transaction) has run.
        let write_txn = db
            .begin_write()
            .expect("failed to begin redb write transaction");
        write_txn
            .open_table(DEPOSITS)
            .expect("failed to create deposits table");
        write_txn
            .commit()
            .expect("failed to commit deposits table creation");

        Ok(Self { db, _dir: dir })
    }

    fn with_write_table(&mut self, f: impl FnOnce(&mut redb::Table<u32, &[u8]>)) {
        let mut write_txn = self
            .db
            .begin_write()
            .expect("failed to begin redb write transaction");
        write_txn.set_durability(redb::Durability::None);
        {
            let mut table = write_txn
                .open_table(DEPOSITS)
                .expect("failed to open deposits table");
            f(&mut table);
        }
        write_txn.commit().expect("failed to commit deposit write");
    }
}

impl DepositStore for LiveDepositStore {
    fn insert(&mut self, tx: TxId, record: DepositRecord) {
        self.with_write_table(|table| {
            table
                .insert(tx.into_inner(), &encode(&record)[..])
                .expect("failed to insert deposit record");
        });
    }

    fn get(&self, tx: TxId) -> Option<DepositRecord> {
        let read_txn = self
            .db
            .begin_read()
            .expect("failed to begin redb read transaction");
        let table = read_txn
            .open_table(DEPOSITS)
            .expect("failed to open deposits table");
        table
            .get(tx.into_inner())
            .expect("failed to read deposit record")
            .map(|guard| decode(guard.value()))
    }

    fn set_state(&mut self, tx: TxId, state: DisputeState) {
        self.with_write_table(|table| {
            let existing = table
                .get(tx.into_inner())
                .expect("failed to read deposit record")
                .map(|guard| decode(guard.value()));
            if let Some(mut record) = existing {
                record.state = state;
                table
                    .insert(tx.into_inner(), &encode(&record)[..])
                    .expect("failed to update deposit record");
            }
        });
    }
}

fn encode(record: &DepositRecord) -> [u8; 19] {
    let mut bytes = [0u8; 19];
    bytes[0..2].copy_from_slice(&record.client.into_inner().to_le_bytes());
    bytes[2..18].copy_from_slice(&record.amount.into_inner().serialize());
    bytes[18] = match record.state {
        DisputeState::NeverDisputed => 0,
        DisputeState::Disputed => 1,
        DisputeState::Settled => 2,
    };
    bytes
}

fn decode(bytes: &[u8]) -> DepositRecord {
    let client = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
    let amount = Decimal::deserialize(bytes[2..18].try_into().unwrap());
    let state = match bytes[18] {
        0 => DisputeState::NeverDisputed,
        1 => DisputeState::Disputed,
        2 => DisputeState::Settled,
        other => panic!("corrupt DepositRecord state byte: {other}"),
    };
    DepositRecord {
        client: ClientId::new(client),
        amount: Amount::new(amount),
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Amount, ClientId};
    use rust_decimal::Decimal;

    fn record(client: u16, amount: &str) -> DepositRecord {
        record_with_state(client, amount, DisputeState::NeverDisputed)
    }

    fn record_with_state(client: u16, amount: &str, state: DisputeState) -> DepositRecord {
        DepositRecord {
            client: ClientId::new(client),
            amount: Amount::new(amount.parse::<Decimal>().unwrap()),
            state,
        }
    }

    #[test]
    fn encode_then_decode_roundtrips_for_every_dispute_state_and_precise_amounts() {
        let records = [
            record_with_state(1, "5.0", DisputeState::NeverDisputed),
            record_with_state(2, "10.5", DisputeState::Disputed),
            record_with_state(3, "0.0001", DisputeState::Settled),
            record_with_state(4, "1.2345", DisputeState::NeverDisputed),
        ];

        for r in records {
            assert_eq!(decode(&encode(&r)), r);
        }
    }

    fn deposit_store_contract<S: DepositStore>(mut store: S) {
        let rec = record(1, "5.1234");
        store.insert(TxId::new(1), rec);
        assert_eq!(store.get(TxId::new(1)), Some(rec));

        assert_eq!(
            store.get(TxId::new(1)).unwrap().state,
            DisputeState::NeverDisputed
        );
        store.set_state(TxId::new(1), DisputeState::Disputed);
        assert_eq!(
            store.get(TxId::new(1)).unwrap().state,
            DisputeState::Disputed
        );
        store.set_state(TxId::new(1), DisputeState::Settled);
        assert_eq!(
            store.get(TxId::new(1)).unwrap().state,
            DisputeState::Settled
        );

        store.set_state(TxId::new(999), DisputeState::Disputed);
        assert_eq!(store.get(TxId::new(999)), None);

        assert_eq!(store.get(TxId::new(42)), None);
    }

    #[test]
    fn in_memory_satisfies_contract() {
        deposit_store_contract(InMemoryDepositStore::default());
    }

    #[test]
    fn live_satisfies_contract() {
        deposit_store_contract(LiveDepositStore::new().unwrap());
    }

    #[test]
    fn live_get_on_brand_new_store_returns_none_without_panicking() {
        let store = LiveDepositStore::new().unwrap();
        assert_eq!(store.get(TxId::new(1)), None);
    }

    #[test]
    fn live_store_holds_up_at_moderate_scale() {
        let mut store = LiveDepositStore::new().unwrap();
        let mut expected = std::collections::HashMap::new();

        for i in 0..5000u32 {
            let client = (i % 500) as u16;
            let amount = Decimal::new(i as i64 * 1234, 4);
            let state = match i % 3 {
                0 => DisputeState::NeverDisputed,
                1 => DisputeState::Disputed,
                _ => DisputeState::Settled,
            };
            let rec = record_with_state(client, &amount.to_string(), state);
            store.insert(TxId::new(i), rec);
            expected.insert(TxId::new(i), rec);
        }

        for (tx, rec) in expected {
            assert_eq!(store.get(tx), Some(rec));
        }
    }
}
