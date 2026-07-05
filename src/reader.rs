use crate::error::ReaderError;
use crate::model::{Entry, RawEntry};
use std::path::Path;

pub fn read_entries(
    path: &Path,
) -> Result<impl Iterator<Item = Result<Entry, ReaderError>>, ReaderError> {
    let mut rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|source| ReaderError::Open {
            path: path.to_path_buf(),
            source,
        })?;

    let headers = rdr.headers().map_err(ReaderError::Csv)?.clone();

    // into_records (not into_deserialize) so we can read each record's
    // position before consuming it, for accurate row numbers in errors.
    let iter = rdr.into_records().map(move |result| {
        let record = result.map_err(ReaderError::Csv)?;
        let row = record.position().map(|p| p.line()).unwrap_or(0);

        let raw: RawEntry = record
            .deserialize(Some(&headers))
            .map_err(ReaderError::Csv)?;

        Entry::try_from(raw).map_err(|source| ReaderError::Parse { row, source })
    });

    Ok(iter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Amount, ClientId, TxId, TxType};
    use rust_decimal::Decimal;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn write_temp_csv(contents: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "octo-ledger-reader-test-{}-{}.csv",
            std::process::id(),
            n
        ));
        std::fs::write(&path, contents).expect("failed to write temp csv");
        path
    }

    #[test]
    fn well_formed_csv_parses_in_order() {
        let path = write_temp_csv(
            "type,client,tx,amount\n\
             deposit,1,1,1.0\n\
             withdrawal,1,2,0.5\n\
             dispute,1,3,\n",
        );

        let entries: Vec<Entry> = read_entries(&path)
            .expect("opening a valid file should succeed")
            .collect::<Result<_, _>>()
            .expect("all rows should parse");

        std::fs::remove_file(&path).ok();

        assert_eq!(
            entries,
            vec![
                Entry {
                    tx_type: TxType::Deposit,
                    client: ClientId::new(1),
                    tx: TxId::new(1),
                    amount: Some(Amount::new(Decimal::new(10, 1))),
                },
                Entry {
                    tx_type: TxType::Withdrawal,
                    client: ClientId::new(1),
                    tx: TxId::new(2),
                    amount: Some(Amount::new(Decimal::new(5, 1))),
                },
                Entry {
                    tx_type: TxType::Dispute,
                    client: ClientId::new(1),
                    tx: TxId::new(3),
                    amount: None,
                },
            ]
        );
    }

    #[test]
    fn bad_transaction_type_yields_a_csv_error_for_that_row_only() {
        let path = write_temp_csv(
            "type,client,tx,amount\n\
             deposit,1,1,1.0\n\
             unknowntype,2,2,\n",
        );

        let results: Vec<Result<Entry, ReaderError>> = read_entries(&path)
            .expect("opening a valid file should succeed")
            .collect();

        std::fs::remove_file(&path).ok();

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(matches!(results[1], Err(ReaderError::Csv(_))));
    }

    #[test]
    fn parse_error_carries_the_actual_row_number() {
        let path = write_temp_csv(
            "type,client,tx,amount\n\
             deposit,1,1,1.0\n\
             deposit,2,2,\n",
        );

        let results: Vec<Result<Entry, ReaderError>> = read_entries(&path)
            .expect("opening a valid file should succeed")
            .collect();

        std::fs::remove_file(&path).ok();

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        match &results[1] {
            Err(ReaderError::Parse { row, .. }) => {
                assert_eq!(*row, 3);
            }
            other => panic!("expected ReaderError::Parse, got {other:?}"),
        }
    }

    #[test]
    fn nonexistent_path_fails_fast_before_iterating() {
        let path = std::env::temp_dir().join("octo-ledger-reader-test-does-not-exist.csv");
        std::fs::remove_file(&path).ok();

        let result = read_entries(&path);

        assert!(matches!(result, Err(ReaderError::Open { .. })));
    }
}
