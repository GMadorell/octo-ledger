use crate::model::TxType;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid amount {0:?}")]
    Amount(String),
    #[error("unknown transaction type {0:?}")]
    TransactionType(String),
    #[error("missing amount for {0:?} transaction")]
    MissingAmount(TxType),
}

#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("could not open input file {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },
    #[error("malformed CSV")]
    Csv(#[source] csv::Error),
    #[error("could not parse row {row}")]
    Parse {
        row: u64,
        #[source]
        source: ParseError,
    },
    #[error("could not write output")]
    Write(#[source] csv::Error),
}
