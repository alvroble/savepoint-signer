use std::io::Write;

fn main() {
    let report = signer_probe::run().expect("fixed signing vector");
    eprintln!("{}", report.address);
    std::io::stdout().write_all(&report.signed_psbt).unwrap();
}
