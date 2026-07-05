use crate::model::{Amount, Entry, Ledger, TxType};

pub fn run(entries: impl Iterator<Item = Entry>) -> Ledger {
    let mut ledger = Ledger::new();

    for entry in entries {
        // TODO: dispute/resolve/chargeback need to look up a past tx's
        // amount by TxId, which means retaining prior transactions in
        // memory — reintroducing the O(all-transactions) footprint this
        // streaming fold avoids. Deferred deliberately.
        match entry.tx_type {
            TxType::Deposit => {
                let Some(amount) = entry.amount else {
                    continue;
                };
                let account = ledger.entry(entry.client);
                account.available =
                    Amount::new(account.available.into_inner() + amount.into_inner());
                account.total = Amount::new(account.total.into_inner() + amount.into_inner());
            }
            TxType::Withdrawal => {
                let Some(amount) = entry.amount else {
                    continue;
                };
                let account = ledger.entry(entry.client);
                account.available =
                    Amount::new(account.available.into_inner() - amount.into_inner());
                account.total = Amount::new(account.total.into_inner() - amount.into_inner());
            }
            TxType::Dispute | TxType::Resolve | TxType::Chargeback => {}
        }
    }

    ledger
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClientId, TxId};
    use rust_decimal::Decimal;

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

    fn amt(s: &str) -> Amount {
        Amount::new(s.parse::<Decimal>().unwrap())
    }

    #[test]
    fn single_deposit_produces_matching_available_and_total() {
        let ledger = run(vec![deposit(1, 1, "1.5")].into_iter());

        let accounts: Vec<_> = ledger.accounts().collect();
        assert_eq!(accounts.len(), 1);
        let account = accounts[0];
        assert_eq!(account.available, amt("1.5"));
        assert_eq!(account.total, amt("1.5"));
        assert_eq!(account.held, amt("0"));
        assert!(!account.locked);
    }

    #[test]
    fn two_deposits_for_same_client_sum_correctly() {
        let ledger = run(vec![deposit(1, 1, "1.5"), deposit(1, 2, "2.25")].into_iter());

        let accounts: Vec<_> = ledger.accounts().collect();
        assert_eq!(accounts.len(), 1);
        let account = accounts[0];
        assert_eq!(account.available, amt("3.75"));
        assert_eq!(account.total, amt("3.75"));
    }

    #[test]
    fn deposits_across_two_clients_produce_independent_accounts() {
        let ledger = run(vec![deposit(1, 1, "1.0"), deposit(2, 2, "5.0")].into_iter());

        let client1 = ledger
            .accounts()
            .find(|a| a.client == ClientId::new(1))
            .expect("client 1 account should exist");
        let client2 = ledger
            .accounts()
            .find(|a| a.client == ClientId::new(2))
            .expect("client 2 account should exist");

        assert_eq!(client1.available, amt("1.0"));
        assert_eq!(client2.available, amt("5.0"));
    }

    #[test]
    fn deposit_followed_by_withdrawal_nets_out() {
        let ledger = run(vec![deposit(1, 1, "5.0"), withdrawal(1, 2, "1.5")].into_iter());

        let accounts: Vec<_> = ledger.accounts().collect();
        assert_eq!(accounts.len(), 1);
        let account = accounts[0];
        assert_eq!(account.available, amt("3.5"));
        assert_eq!(account.total, amt("3.5"));
        assert_eq!(account.held, amt("0"));
    }
}
