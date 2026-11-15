//! Fourth review of card-guards (523e46524): a deeply nested script (oxc parses and walks recursively), depth from
//! RZ_DEPTH, on a thread with the default 2 MB stack as tokio's workers have. A stack overflow aborts the process.

#[test]
fn rz6_a_deeply_nested_script_neither_crashes_nor_passes() {
    let depth: usize = std::env::var("RZ_DEPTH").ok().and_then(|d| d.parse().ok()).unwrap_or(1000);
    let script = format!("return {}1{}", "[".repeat(depth), "]".repeat(depth));
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || bipbridge::browser_mcp::script_card_refusal(&script, None).map(|s| s[..40].to_string()))
        .unwrap();
    println!("depth {depth}: {:?}", handle.join().expect("no panic"));
}
