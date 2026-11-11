// F0 only wires `--version`; the command surface arrives in F1.
fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version" | "-v") => println!("{}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("spendbranch: not implemented yet");
            std::process::exit(2);
        }
    }
}
