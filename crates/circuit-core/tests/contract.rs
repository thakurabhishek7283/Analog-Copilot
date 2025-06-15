//! The committed JSON Schemas in `contract/schema/` must match the Rust types exactly.
//! Regenerate with `cargo run -p circuit-core --example export_schema`.

use std::fs;
use std::path::PathBuf;

use circuit_core::schema::{all_schemas, render};

#[test]
fn committed_schemas_are_up_to_date() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contract/schema");
    let mut stale = Vec::new();
    for (name, schema) in all_schemas() {
        let on_disk = fs::read_to_string(dir.join(name)).unwrap_or_default().replace("\r\n", "\n");
        if on_disk != render(&schema) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "stale contract schemas {stale:?}; run `cargo run -p circuit-core --example export_schema`"
    );
}
