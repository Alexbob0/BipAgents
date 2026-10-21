//! `bipbridge [serve|check|genkey|qr|peer-token|browser|shares-mounts|console|spaces|adopt]` (Python:
//! `python -m bipbridge …`). The commands live in `bipbridge::main_cli`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(bipbridge::main_cli::main(args));
}
