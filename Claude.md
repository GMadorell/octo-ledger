# octo-ledger

Rust CLI that reads a transactions CSV and writes a per-client ledger CSV to stdout.

See `README.md` for behavior, formats, architecture, and build/test instructions — it's the real, kept-up-to-date documentation.

## Structure

```
Cargo.toml
src/
  main.rs
  model.rs
  error.rs
  parser.rs
  reader.rs
  engine.rs
  printer.rs
examples/
tests/
  integration_test.rs
```
