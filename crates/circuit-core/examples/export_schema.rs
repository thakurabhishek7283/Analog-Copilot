//! Write the contract JSON Schemas to `contract/schema/`.
//! `cargo run -p circuit-core --example export_schema`

use std::fs;
use std::path::PathBuf;

use circuit_core::schema::{all_schemas, render};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contract/schema");
    fs::create_dir_all(&dir).expect("create contract/schema");
    for (name, schema) in all_schemas() {
        fs::write(dir.join(name), render(&schema)).expect("write schema");
        println!("wrote contract/schema/{name}");
    }
}
