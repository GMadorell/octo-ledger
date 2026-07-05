use crate::model::{DepositRecord, DisputeState, TxId};

pub trait DepositStore {
    fn insert(&mut self, tx: TxId, record: DepositRecord);
    fn get(&self, tx: TxId) -> Option<DepositRecord>;
    fn set_state(&mut self, tx: TxId, state: DisputeState);
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryDepositStore {
    records: std::collections::HashMap<TxId, DepositRecord>,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Amount, ClientId};
    use rust_decimal::Decimal;

    fn record(client: u16, amount: &str) -> DepositRecord {
        DepositRecord {
            client: ClientId::new(client),
            amount: Amount::new(amount.parse::<Decimal>().unwrap()),
            state: DisputeState::NeverDisputed,
        }
    }

    #[test]
    fn insert_then_get_returns_the_same_record() {
        let mut store = InMemoryDepositStore::default();
        let rec = record(1, "5.0");

        store.insert(TxId::new(1), rec);

        assert_eq!(store.get(TxId::new(1)), Some(rec));
    }

    #[test]
    fn set_state_advances_the_state_of_an_existing_record() {
        let mut store = InMemoryDepositStore::default();
        store.insert(TxId::new(1), record(1, "5.0"));

        store.set_state(TxId::new(1), DisputeState::Disputed);

        let updated = store.get(TxId::new(1)).unwrap();
        assert_eq!(updated.state, DisputeState::Disputed);
    }

    #[test]
    fn get_on_an_unknown_tx_id_returns_none() {
        let store = InMemoryDepositStore::default();

        assert_eq!(store.get(TxId::new(1)), None);
    }
}
