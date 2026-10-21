//! `bip-browser-mcp <config.json>` (Python: `python3 browser_mcp.py <config.json>`): an agent's browser tools,
//! started when used. The logic lives in `bipbridge::browser_mcp`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(bipbridge::browser_mcp::main(args));
}
