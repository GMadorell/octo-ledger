use crate::error::ParseError;
use crate::model::{Amount, ClientId, Entry, RawEntry, TxId, TxType};

impl TryFrom<RawEntry> for Entry {
    type Error = ParseError;

    fn try_from(raw: RawEntry) -> Result<Self, Self::Error> {
        let amount = match raw.r#type {
            TxType::Deposit | TxType::Withdrawal => match raw.amount {
                Some(decimal) => Some(Amount::new(decimal)),
                None => return Err(ParseError::MissingAmount(raw.r#type)),
            },
            // Any amount on these rows references another tx's value, not
            // their own — silently dropped rather than treated as an error.
            TxType::Dispute | TxType::Resolve | TxType::Chargeback => None,
        };

        Ok(Entry {
            tx_type: raw.r#type,
            client: ClientId::new(raw.client),
            tx: TxId::new(raw.tx),
            amount,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn raw(tx_type: TxType, amount: Option<Decimal>) -> RawEntry {
        RawEntry {
            r#type: tx_type,
            client: 1,
            tx: 42,
            amount,
        }
    }

    #[test]
    fn deposit_with_amount_maps_fields() {
        let entry = Entry::try_from(raw(TxType::Deposit, Some(Decimal::new(105, 2)))).unwrap();

        assert_eq!(entry.tx_type, TxType::Deposit);
        assert_eq!(entry.client, ClientId::new(1));
        assert_eq!(entry.tx, TxId::new(42));
        assert_eq!(entry.amount, Some(Amount::new(Decimal::new(105, 2))));
    }

    #[test]
    fn withdrawal_with_amount_is_ok() {
        let entry = Entry::try_from(raw(TxType::Withdrawal, Some(Decimal::new(50, 0)))).unwrap();

        assert_eq!(entry.amount, Some(Amount::new(Decimal::new(50, 0))));
    }

    #[test]
    fn deposit_without_amount_is_missing_amount_error() {
        let err = Entry::try_from(raw(TxType::Deposit, None)).unwrap_err();

        assert!(matches!(err, ParseError::MissingAmount(TxType::Deposit)));
    }

    #[test]
    fn dispute_without_amount_is_ok_with_no_amount() {
        let entry = Entry::try_from(raw(TxType::Dispute, None)).unwrap();

        assert_eq!(entry.amount, None);
    }

    #[test]
    fn dispute_with_amount_present_is_silently_dropped() {
        let entry = Entry::try_from(raw(TxType::Dispute, Some(Decimal::new(999, 2)))).unwrap();

        assert_eq!(entry.amount, None);
    }
}
