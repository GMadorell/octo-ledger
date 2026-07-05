mod engine;
mod error;
mod model;
mod parser;
mod printer;
mod reader;
mod store;

use clap::Parser;
use error::ReaderError;
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

#[derive(Parser)]
struct Cli {
    input: PathBuf,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                eprintln!("  caused by: {s}");
                src = s.source();
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), ReaderError> {
    let cli = Cli::parse();

    let entries = reader::read_entries(&cli.input)?;

    // Bridges reader's Result<Entry, _> stream into engine's Entry stream
    // lazily (no Vec collection), stopping at the first error.
    let first_error: Rc<RefCell<Option<ReaderError>>> = Rc::new(RefCell::new(None));
    let error_slot = Rc::clone(&first_error);
    let mut entries = entries;
    let bridged = std::iter::from_fn(move || match entries.next() {
        Some(Ok(entry)) => Some(entry),
        Some(Err(e)) => {
            *error_slot.borrow_mut() = Some(e);
            None
        }
        None => None,
    });

    let ledger = engine::run(bridged);

    if let Some(err) = first_error.borrow_mut().take() {
        return Err(err);
    }

    printer::write_ledger(&ledger, std::io::stdout()).map_err(ReaderError::Write)?;

    Ok(())
}
