//! Compile `registry/` (YAML) into the JSON bundle the browser and API load (LLD §12 step 3).
//! `cargo run -p circuit-core --example bundle_registry [-- <out_dir>]`
//! Writes `<out_dir>/registry-<version>.json` (default out_dir: `target/registry`).

#[path = "support/registry_dir.rs"]
mod registry_dir;

use std::fs;
use std::path::PathBuf;

fn main() {
    let root = registry_dir::repo_root();
    let reg = registry_dir::load(&root.join("registry"));
    let out_dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| root.join("target/registry"));
    fs::create_dir_all(&out_dir).expect("create output dir");
    let path = out_dir.join(format!("registry-{}.json", reg.version));
    fs::write(&path, serde_json::to_string(&reg).expect("serialize")).expect("write bundle");
    println!("{}", path.display());
}
