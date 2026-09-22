//! Emulator-only adapter to the production Terminal, with public test data.
use signer_probe::terminal::{PsbtFile, Terminal, PSBT_LISTING, PSBT_LOADING};
use std::io::{self, BufRead, Write};
fn main() {
    let mut terminal = Terminal::default();
    for line in io::stdin().lock().lines() {
        let c: u8 = line.unwrap().parse().unwrap();
        terminal.command(c);
        if terminal.state == PSBT_LISTING {
            terminal.set_files(vec![
                PsbtFile {
                    name: "invoice-september.psbt".into(),
                    alias: "INVOIC~1.PSB".into(),
                    size: 499,
                },
                PsbtFile {
                    name: "savings-transfer.psbt".into(),
                    alias: "SAVING~1.PSB".into(),
                    size: 1024,
                },
            ]);
        }
        if terminal.state == PSBT_LOADING {
            terminal.load_psbt(include_bytes!("../tests/fixtures/game-main.psb"));
        }
        if terminal.state == 3 {
            terminal.finish_recovery();
        }
        let mut out = terminal.report().to_vec();
        out.extend_from_slice(&terminal.qr_buffer);
        for b in out {
            print!("{b:02x}");
        }
        println!();
        io::stdout().flush().unwrap();
    }
}
