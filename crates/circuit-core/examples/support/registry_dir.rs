//! Read `registry/` from disk (dev tools and tests only; the library itself does no I/O).

use std::fs;
use std::path::{Path, PathBuf};

use circuit_core::Registry;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Load `<root>/manifest.yaml` and every `<root>/parts/*.yaml` through the real YAML loader.
pub fn load(root: &Path) -> Registry {
    #[derive(serde::Deserialize)]
    struct Manifest {
        version: String,
    }
    let manifest = fs::read_to_string(root.join("manifest.yaml")).expect("registry/manifest.yaml");
    let m: Manifest = serde_norway::from_str(&manifest).expect("manifest has a version");
    let mut files: Vec<PathBuf> = fs::read_dir(root.join("parts"))
        .expect("registry/parts")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    files.sort();
    let docs: Vec<(String, String)> = files
        .iter()
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), fs::read_to_string(p).unwrap()))
        .collect();
    Registry::from_yaml_docs(m.version, docs.iter().map(|(n, t)| (n.as_str(), t.as_str())))
        .unwrap_or_else(|errs| panic!("registry failed to load:\n{errs:#?}"))
}
