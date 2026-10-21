// The Python package's data files are embedded (src/assets.rs): rebuild when they change.
fn main() {
    for path in ["console", "static", "skills", "agent_templates.yaml", "tool_catalog.yaml"] {
        println!("cargo:rerun-if-changed=../bridge/bipbridge/{path}");
    }
}
