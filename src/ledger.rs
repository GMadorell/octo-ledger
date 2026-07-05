use crate::model::{Account, ClientId};
use std::collections::HashMap;

pub trait LedgerStore {
    fn get(&self, client: ClientId) -> Option<Account>;
    fn upsert(&mut self, client: ClientId, account: Account);
    fn accounts(&self) -> impl Iterator<Item = &Account>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryLedger {
    accounts: HashMap<ClientId, Account>,
}

impl LedgerStore for InMemoryLedger {
    fn get(&self, client: ClientId) -> Option<Account> {
        self.accounts.get(&client).cloned()
    }

    fn upsert(&mut self, client: ClientId, account: Account) {
        self.accounts.insert(client, account);
    }

    fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }
}
