use std::collections::VecDeque;

pub const SCALE_BULK_CLIENTS: u16 = 25;
const SCALE_TOTAL_ROWS: usize = 50_000;
const CONTROL_CLIENT_A: u16 = 9001;
const CONTROL_CLIENT_B: u16 = 9002;

fn format_cents(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn push_row(csv: &mut String, tx_type: &str, client: u16, tx: u32, amount: Option<&str>) {
    csv.push_str(tx_type);
    csv.push(',');
    csv.push_str(&client.to_string());
    csv.push(',');
    csv.push_str(&tx.to_string());
    csv.push(',');
    if let Some(a) = amount {
        csv.push_str(a);
    }
    csv.push('\n');
}

#[derive(Default)]
struct BulkClientState {
    available_cents: i64,
    locked: bool,
    open_deposits: VecDeque<(u32, i64)>,
    disputed_deposits: VecDeque<(u32, i64)>,
}

fn emit_deposit(
    csv: &mut String,
    next_tx: &mut u32,
    state: &mut BulkClientState,
    client_id: u16,
    amount_cents: i64,
) {
    let tx = *next_tx;
    *next_tx += 1;
    push_row(
        csv,
        "deposit",
        client_id,
        tx,
        Some(&format_cents(amount_cents)),
    );
    state.available_cents += amount_cents;
    state.open_deposits.push_back((tx, amount_cents));
    if state.open_deposits.len() > 40 {
        state.open_deposits.pop_front();
    }
}

fn fallback_deposit(
    csv: &mut String,
    next_tx: &mut u32,
    state: &mut BulkClientState,
    client_id: u16,
    i: usize,
) {
    let amount_cents = 100 + (i as i64 % 500);
    emit_deposit(csv, next_tx, state, client_id, amount_cents);
}

pub fn build_scale_csv() -> String {
    let mut csv = String::with_capacity(SCALE_TOTAL_ROWS * 24);
    csv.push_str("type,client,tx,amount\n");

    let mut next_tx: u32 = 1;

    let a_dep1 = next_tx;
    next_tx += 1;
    push_row(
        &mut csv,
        "deposit",
        CONTROL_CLIENT_A,
        a_dep1,
        Some("100.00"),
    );
    let a_dep2 = next_tx;
    next_tx += 1;
    push_row(&mut csv, "deposit", CONTROL_CLIENT_A, a_dep2, Some("50.00"));
    push_row(&mut csv, "dispute", CONTROL_CLIENT_A, a_dep1, None);
    push_row(&mut csv, "resolve", CONTROL_CLIENT_A, a_dep1, None);
    let a_wd1 = next_tx;
    next_tx += 1;
    push_row(
        &mut csv,
        "withdrawal",
        CONTROL_CLIENT_A,
        a_wd1,
        Some("30.00"),
    );

    let b_dep1 = next_tx;
    next_tx += 1;
    push_row(
        &mut csv,
        "deposit",
        CONTROL_CLIENT_B,
        b_dep1,
        Some("200.00"),
    );
    push_row(&mut csv, "dispute", CONTROL_CLIENT_B, b_dep1, None);
    push_row(&mut csv, "chargeback", CONTROL_CLIENT_B, b_dep1, None);
    let b_dep2 = next_tx;
    next_tx += 1;
    push_row(
        &mut csv,
        "deposit",
        CONTROL_CLIENT_B,
        b_dep2,
        Some("999.00"),
    );

    let control_rows = 5 + 4;
    let bulk_rows = SCALE_TOTAL_ROWS - control_rows;

    let mut states: Vec<BulkClientState> = (0..SCALE_BULK_CLIENTS)
        .map(|_| BulkClientState::default())
        .collect();

    for i in 0..bulk_rows {
        let client_idx = i % SCALE_BULK_CLIENTS as usize;
        let client_id = client_idx as u16 + 1;
        let bucket = i % 10;
        let state = &mut states[client_idx];

        match bucket {
            0..=4 => {
                let amount_cents = 100 + ((i as i64 * 37 + client_idx as i64 * 13) % 5000);
                emit_deposit(&mut csv, &mut next_tx, state, client_id, amount_cents);
            }
            5..=6 => {
                let raw = 50 + ((i as i64 * 13) % 2000);
                let amount_cents = raw.min(state.available_cents);
                if !state.locked && amount_cents > 0 {
                    let tx = next_tx;
                    next_tx += 1;
                    push_row(
                        &mut csv,
                        "withdrawal",
                        client_id,
                        tx,
                        Some(&format_cents(amount_cents)),
                    );
                    state.available_cents -= amount_cents;
                } else {
                    fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                }
            }
            7 => {
                let candidate = state.open_deposits.front().copied();
                match candidate {
                    Some((tx, amt)) if !state.locked && amt <= state.available_cents => {
                        state.open_deposits.pop_front();
                        push_row(&mut csv, "dispute", client_id, tx, None);
                        state.available_cents -= amt;
                        state.disputed_deposits.push_back((tx, amt));
                    }
                    _ => {
                        fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                    }
                }
            }
            8 => {
                if !state.locked {
                    if let Some((tx, amt)) = state.disputed_deposits.pop_front() {
                        push_row(&mut csv, "resolve", client_id, tx, None);
                        state.available_cents += amt;
                    } else {
                        fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                    }
                } else {
                    fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                }
            }
            9 => {
                if !state.locked {
                    if let Some((tx, _amt)) = state.disputed_deposits.pop_front() {
                        push_row(&mut csv, "chargeback", client_id, tx, None);
                        state.locked = true;
                    } else {
                        fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                    }
                } else {
                    fallback_deposit(&mut csv, &mut next_tx, state, client_id, i);
                }
            }
            _ => unreachable!("i % 10 is always in 0..=9"),
        }
    }

    csv
}
