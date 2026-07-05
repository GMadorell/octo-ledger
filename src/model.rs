use nutype::nutype;
use rust_decimal::Decimal;
use std::collections::HashMap;

#[nutype(derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash))]
pub struct ClientId(u16);

#[nutype(derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash))]
pub struct TxId(u32);

#[nutype(derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq))]
pub struct Amount(Decimal);

impl std::ops::Add for Amount {
    type Output = Amount;
    fn add(self, rhs: Amount) -> Amount {
        Amount::new(self.into_inner() + rhs.into_inner())
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;
    fn sub(self, rhs: Amount) -> Amount {
        Amount::new(self.into_inner() - rhs.into_inner())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TxType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    NeverDisputed,
    Disputed,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepositRecord {
    pub client: ClientId,
    pub amount: Amount,
    pub state: DisputeState,
}

#[derive(Debug, serde::Deserialize)]
pub struct RawEntry {
    #[serde(rename = "type")]
    pub r#type: TxType,
    pub client: u16,
    pub tx: u32,
    pub amount: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub tx_type: TxType,
    pub client: ClientId,
    pub tx: TxId,
    pub amount: Option<Amount>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Account {
    pub client: ClientId,
    pub available: Amount,
    pub held: Amount,
    pub total: Amount,
    pub locked: bool,
}

impl Account {
    pub fn new(client: ClientId) -> Self {
        Self {
            client,
            available: Amount::new(Decimal::ZERO),
            held: Amount::new(Decimal::ZERO),
            total: Amount::new(Decimal::ZERO),
            locked: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Ledger {
    accounts: HashMap<ClientId, Account>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    pub fn entry(&mut self, client: ClientId) -> &mut Account {
        self.accounts
            .entry(client)
            .or_insert_with(|| Account::new(client))
    }

    pub fn get_mut(&mut self, client: ClientId) -> Option<&mut Account> {
        self.accounts.get_mut(&client)
    }
}
