// The bridge's data files are embedded (src/assets.rs): rebuild when they change.
fn main() {
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=../docs/migrate");
}
