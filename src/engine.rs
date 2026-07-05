#[cfg(test)]
use crate::ledger::InMemoryLedger;
use crate::ledger::LedgerStore;
use crate::model::{Account, DepositRecord, DisputeState, Entry, TxType};
use crate::store::DepositStore;
#[cfg(test)]
use crate::store::InMemoryDepositStore;

#[cfg(test)]
pub fn run(entries: impl Iterator<Item = Entry>) -> InMemoryLedger {
    run_with_stores(
        entries,
        InMemoryDepositStore::default(),
        InMemoryLedger::default(),
    )
}

pub fn run_with_stores<D: DepositStore, L: LedgerStore>(
    entries: impl Iterator<Item = Entry>,
    mut store: D,
    mut ledger: L,
) -> L {
    for entry in entries {
        match entry.tx_type {
            TxType::Deposit => {
                let Some(amount) = entry.amount else {
                    continue;
                };
                let mut account = ledger
                    .get(entry.client)
                    .unwrap_or_else(|| Account::new(entry.client));
                if account.locked {
                    continue;
                }
                account.available = account.available + amount;
                account.total = account.total + amount;
                ledger.upsert(entry.client, account);
                store.insert(
                    entry.tx,
                    DepositRecord {
                        client: entry.client,
                        amount,
                        state: DisputeState::NeverDisputed,
                    },
                );
            }
            TxType::Withdrawal => {
                let Some(amount) = entry.amount else {
                    continue;
                };
                let Some(mut account) = ledger.get(entry.client) else {
                    continue;
                };
                if account.locked || account.available.into_inner() < amount.into_inner() {
                    continue;
                }
                account.available = account.available - amount;
                account.total = account.total - amount;
                ledger.upsert(entry.client, account);
            }
            TxType::Dispute => {
                let Some((record, mut account)) =
                    validate_dispute_target(&ledger, &store, &entry, DisputeState::NeverDisputed)
                else {
                    continue;
                };
                if account.available.into_inner() < record.amount.into_inner() {
                    continue;
                }
                account.available = account.available - record.amount;
                account.held = account.held + record.amount;
                store.set_state(entry.tx, DisputeState::Disputed);
                ledger.upsert(record.client, account);
            }
            TxType::Resolve => {
                let Some((record, mut account)) =
                    validate_dispute_target(&ledger, &store, &entry, DisputeState::Disputed)
                else {
                    continue;
                };
                account.available = account.available + record.amount;
                account.held = account.held - record.amount;
                store.set_state(entry.tx, DisputeState::Settled);
                ledger.upsert(record.client, account);
            }
            TxType::Chargeback => {
                let Some((record, mut account)) =
                    validate_dispute_target(&ledger, &store, &entry, DisputeState::Disputed)
                else {
                    continue;
                };
                account.held = account.held - record.amount;
                account.total = account.total - record.amount;
                store.set_state(entry.tx, DisputeState::Settled);
                account.locked = true;
                ledger.upsert(record.client, account);
            }
        }
    }

    ledger
}

fn validate_dispute_target<S: DepositStore, L: LedgerStore>(
    ledger: &L,
    store: &S,
    entry: &Entry,
    expected_state: DisputeState,
) -> Option<(DepositRecord, Account)> {
    let record = store.get(entry.tx)?;
    if record.client != entry.client {
        return None;
    }
    let account = ledger
        .get(record.client)
        .unwrap_or_else(|| Account::new(record.client));
    if account.locked || record.state != expected_state {
        return None;
    }
    Some((record, account))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Amount, ClientId, TxId};
    use rust_decimal::Decimal;

    fn all_accounts(ledger: &InMemoryLedger) -> Vec<crate::model::Account> {
        ledger.accounts().cloned().collect()
    }

    fn deposit(client: u16, tx: u32, amount: &str) -> Entry {
        Entry {
            tx_type: TxType::Deposit,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: Some(Amount::new(amount.parse::<Decimal>().unwrap())),
        }
    }

    fn withdrawal(client: u16, tx: u32, amount: &str) -> Entry {
        Entry {
            tx_type: TxType::Withdrawal,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: Some(Amount::new(amount.parse::<Decimal>().unwrap())),
        }
    }

    fn dispute(client: u16, tx: u32) -> Entry {
        Entry {
            tx_type: TxType::Dispute,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: None,
        }
    }

    fn resolve(client: u16, tx: u32) -> Entry {
        Entry {
            tx_type: TxType::Resolve,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: None,
        }
    }

    fn chargeback(client: u16, tx: u32) -> Entry {
        Entry {
            tx_type: TxType::Chargeback,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: None,
        }
    }

    fn amt(s: &str) -> Amount {
        Amount::new(s.parse::<Decimal>().unwrap())
    }

    fn account_for(ledger: &InMemoryLedger, client: u16) -> crate::model::Account {
        all_accounts(ledger)
            .into_iter()
            .find(|a| a.client == ClientId::new(client))
            .expect("account should exist")
    }

    #[test]
    fn single_deposit_produces_matching_available_and_total() {
        let ledger = run(vec![deposit(1, 1, "1.5")].into_iter());

        let accounts = all_accounts(&ledger);
        assert_eq!(accounts.len(), 1);
        let account = &accounts[0];
        assert_eq!(account.available, amt("1.5"));
        assert_eq!(account.total, amt("1.5"));
        assert_eq!(account.held, amt("0"));
        assert!(!account.locked);
    }

    #[test]
    fn two_deposits_for_same_client_sum_correctly() {
        let ledger = run(vec![deposit(1, 1, "1.5"), deposit(1, 2, "2.25")].into_iter());

        let accounts = all_accounts(&ledger);
        assert_eq!(accounts.len(), 1);
        let account = &accounts[0];
        assert_eq!(account.available, amt("3.75"));
        assert_eq!(account.total, amt("3.75"));
    }

    #[test]
    fn deposits_across_two_clients_produce_independent_accounts() {
        let ledger = run(vec![deposit(1, 1, "1.0"), deposit(2, 2, "5.0")].into_iter());

        let accounts = all_accounts(&ledger);
        let client1 = accounts
            .iter()
            .find(|a| a.client == ClientId::new(1))
            .expect("client 1 account should exist");
        let client2 = accounts
            .iter()
            .find(|a| a.client == ClientId::new(2))
            .expect("client 2 account should exist");

        assert_eq!(client1.available, amt("1.0"));
        assert_eq!(client2.available, amt("5.0"));
    }

    #[test]
    fn deposit_followed_by_withdrawal_nets_out() {
        let ledger = run(vec![deposit(1, 1, "5.0"), withdrawal(1, 2, "1.5")].into_iter());

        let accounts = all_accounts(&ledger);
        assert_eq!(accounts.len(), 1);
        let account = &accounts[0];
        assert_eq!(account.available, amt("3.5"));
        assert_eq!(account.total, amt("3.5"));
        assert_eq!(account.held, amt("0"));
    }

    #[test]
    fn dispute_moves_funds_from_available_to_held_leaving_total_unchanged() {
        let ledger = run(vec![deposit(1, 1, "5.0"), dispute(1, 1)].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("0"));
        assert_eq!(account.held, amt("5.0"));
        assert_eq!(account.total, amt("5.0"));
        assert!(!account.locked);
    }

    #[test]
    fn resolve_returns_disputed_funds_to_available_leaving_total_unchanged() {
        let ledger = run(vec![deposit(1, 1, "5.0"), dispute(1, 1), resolve(1, 1)].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("5.0"));
        assert_eq!(account.held, amt("0"));
        assert_eq!(account.total, amt("5.0"));
        assert!(!account.locked);
    }

    #[test]
    fn chargeback_drops_held_and_total_and_locks_the_account() {
        let ledger = run(vec![deposit(1, 1, "5.0"), dispute(1, 1), chargeback(1, 1)].into_iter());

        let acc = account_for(&ledger, 1);
        assert_eq!(
            (acc.available, acc.held, acc.total),
            (amt("0"), amt("0"), amt("0"))
        );
        assert!(acc.locked);
    }

    #[test]
    fn client_mismatch_on_dispute_is_ignored() {
        let ledger = run(vec![deposit(1, 1, "5.0"), dispute(2, 1)].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("5.0"));
        assert_eq!(account.held, amt("0"));
    }

    #[test]
    fn resolve_when_not_under_dispute_is_ignored() {
        let ledger = run(vec![deposit(1, 1, "5.0"), resolve(1, 1)].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("5.0"));
        assert_eq!(account.held, amt("0"));
    }

    #[test]
    fn a_settled_deposit_cannot_be_disputed_again() {
        let ledger = run(vec![
            deposit(1, 1, "5.0"),
            dispute(1, 1),
            resolve(1, 1),
            dispute(1, 1),
        ]
        .into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("5.0"));
        assert_eq!(account.held, amt("0"));
    }

    #[test]
    fn once_locked_further_operations_on_that_client_are_ignored() {
        let ledger = run(vec![
            deposit(1, 1, "5.0"),
            dispute(1, 1),
            chargeback(1, 1),
            deposit(1, 2, "10.0"),
            withdrawal(1, 3, "0.0"),
            dispute(1, 1),
        ]
        .into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("0"));
        assert_eq!(account.held, amt("0"));
        assert_eq!(account.total, amt("0"));
        assert!(account.locked);
    }

    #[test]
    fn funds_short_dispute_is_ignored_and_deposit_remains_disputable() {
        // Withdraw everything so the dispute can't place a hold, then deposit again
        // so funds are available and the same tx can still be disputed successfully.
        let ledger =
            run(vec![deposit(1, 1, "5.0"), withdrawal(1, 2, "5.0"), dispute(1, 1)].into_iter());
        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("0"));
        assert_eq!(account.held, amt("0"));
        assert_eq!(account.total, amt("0"));

        let ledger = run(vec![
            deposit(1, 1, "5.0"),
            withdrawal(1, 2, "5.0"),
            dispute(1, 1),
            deposit(1, 3, "5.0"),
            dispute(1, 1),
        ]
        .into_iter());
        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("0"));
        assert_eq!(account.held, amt("5.0"));
        assert_eq!(account.total, amt("5.0"));
    }

    #[test]
    fn withdrawal_with_insufficient_funds_is_ignored() {
        let ledger = run(vec![deposit(1, 1, "1.0"), withdrawal(1, 2, "5.0")].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("1.0"));
        assert_eq!(account.total, amt("1.0"));
    }

    #[test]
    fn withdrawal_against_unknown_client_creates_no_account() {
        let ledger = run(vec![withdrawal(1, 1, "5.0")].into_iter());

        assert_eq!(all_accounts(&ledger).len(), 0);
    }

    #[test]
    fn dispute_resolve_chargeback_referencing_a_nonexistent_tx_is_ignored() {
        let ledger = run(vec![
            deposit(1, 1, "5.0"),
            dispute(1, 99),
            resolve(1, 99),
            chargeback(1, 99),
        ]
        .into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("5.0"));
        assert_eq!(account.held, amt("0"));
        assert!(!account.locked);
    }

    #[test]
    fn dispute_referencing_a_withdrawals_tx_id_is_ignored() {
        let ledger =
            run(vec![deposit(1, 1, "5.0"), withdrawal(1, 2, "1.0"), dispute(1, 2)].into_iter());

        let account = account_for(&ledger, 1);
        assert_eq!(account.available, amt("4.0"));
        assert_eq!(account.held, amt("0"));
        assert_eq!(account.total, amt("4.0"));
    }
}
