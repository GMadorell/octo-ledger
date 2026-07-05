use crate::ledger::LedgerStore;

pub fn write_ledger<L: LedgerStore, W: std::io::Write>(
    ledger: &L,
    writer: W,
) -> Result<(), csv::Error> {
    // has_headers(false) + a manual write_record ensures the header is
    // written even with zero accounts (serde's auto header only writes on
    // the first serialize() call).
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(writer);

    writer.write_record(["client", "available", "held", "total", "locked"])?;

    for account in ledger.accounts() {
        writer.serialize(account)?;
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entry, TxType};
    use crate::{engine, model::Amount, model::ClientId, model::TxId};
    use rust_decimal::Decimal;

    fn entry(tx_type: TxType, client: u16, tx: u32, amount: Option<&str>) -> Entry {
        Entry {
            tx_type,
            client: ClientId::new(client),
            tx: TxId::new(tx),
            amount: amount.map(|a| Amount::new(a.parse::<Decimal>().unwrap())),
        }
    }

    #[test]
    fn writes_header_and_rows() {
        let entries = vec![
            entry(TxType::Deposit, 1, 1, Some("1.5")),
            entry(TxType::Deposit, 2, 2, Some("2.0")),
        ];
        let ledger = engine::run(entries.into_iter());

        let mut buf = Vec::new();
        write_ledger(&ledger, &mut buf).expect("write_ledger should succeed");
        let output = String::from_utf8(buf).expect("output should be valid utf8");

        assert!(output.contains("client,available,held,total,locked"));
        assert!(output.contains("1,1.5,0,1.5,false"));
        assert!(output.contains("2,2.0,0,2.0,false"));
    }

    #[test]
    fn empty_ledger_writes_only_header() {
        let ledger = engine::run(std::iter::empty::<Entry>());

        let mut buf = Vec::new();
        write_ledger(&ledger, &mut buf).expect("write_ledger should succeed");
        let output = String::from_utf8(buf).expect("output should be valid utf8");

        assert_eq!(output.trim(), "client,available,held,total,locked");
    }
}
