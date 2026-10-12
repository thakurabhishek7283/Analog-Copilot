//! The SPICE import parser (LLD §17 D3) under fuzzing: random text, token soup and mutations of
//! real models never panic; whatever it accepts it re-emits from the tree using only allow-listed
//! cards, and re-emission is a fixed point after one round (what it writes, it reads back to the
//! same text). Every registry model parses and re-emits.

mod common;

use std::collections::BTreeMap;

use circuit_core::spice_import::{SpiceModel, allowed_line, compile, parse};
use common::registry_root;
use proptest::prelude::*;

/// The registry's model files and a few hand-written decks with every construct.
fn corpus() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(registry_root().join("models"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "lib"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    out.sort();
    out.push(
        "\
.param gbw=1meg k={gbw/1k}
.func half(x) {x/2}
.subckt amp a b c d o params: g=10
r1 a b {1meg*half(2)} tc1=0.001
c1 b 0 1p
l1 c d 1u
l2 d 0 2u
k1 l1 l2 0.9
e1 n1 0 a b {g}
g1 0 n2 poly(2) a 0 b 0 0 1m 2m 3m
f1 n3 0 vs 2
h1 n4 0 poly(1) vs 0 5
vs n5 o 0
b1 n5 0 v=min(max(v(n1), -1), 1) + i(vs)*0
egain n6 0 value={v(a,b)*g}
d1 o c dm 2
q1 c b o qn
m1 d c o 0 nm l=1u w=10u
j1 d c o jn
x1 a o sub params: v=2
i1 n6 0 dc 1m
.model dm d(is=1e-14)
.model qn npn(bf=100)
.model nm nmos(level=1 vto=0.7)
.model jn njf(beta=1m)
.ends amp
.subckt sub p q v=1
r1 p q {v*1k}
.ends sub
"
        .into(),
    );
    out
}

/// Every sub-circuit of an accepted text, compiled with its ports as pins.
fn emit_all(text: &str) -> Vec<Vec<String>> {
    let Ok(deck) = parse(text) else { return Vec::new() };
    let mut out = Vec::new();
    for s in &deck.subckts {
        let pins: Vec<String> = s.ports.iter().map(|p| p.to_ascii_lowercase()).collect();
        let mut seen = std::collections::BTreeSet::new();
        if !pins.iter().all(|p| seen.insert(p.clone())) {
            continue;
        }
        let m = SpiceModel {
            text: text.to_string(),
            subckt: s.name.clone(),
            pins: pins.iter().map(|p| (p.clone(), p.clone())).collect(),
        };
        if let Ok(e) = compile(&m, &pins, &BTreeMap::new(), "lib_t") {
            out.push(e.lines);
        }
    }
    out
}

/// Emitted lines are allow-listed, and emitting what was emitted gives it back after one round.
fn check_emitted(lines: &[String]) -> Result<(), TestCaseError> {
    for l in lines {
        prop_assert!(allowed_line(l), "not allowed: {l}");
    }
    let text = format!("{}\n", lines.join("\n"));
    let pins: Vec<String> = lines
        .iter()
        .find(|l| l.starts_with(".subckt lib_t "))
        .map(|l| l.split_whitespace().skip(2).map(str::to_string).collect())
        .unwrap_or_default();
    let m = SpiceModel { text, subckt: "lib_t".into(), pins: pins.iter().map(|p| (p.clone(), p.clone())).collect() };
    let once =
        compile(&m, &pins, &BTreeMap::new(), "lib_t").map_err(|e| TestCaseError::fail(format!("re-reading: {e}")))?;
    let text2 = format!("{}\n", once.lines.join("\n"));
    let m2 = SpiceModel { text: text2, ..m };
    let twice = compile(&m2, &pins, &BTreeMap::new(), "lib_t")
        .map_err(|e| TestCaseError::fail(format!("re-reading again: {e}")))?;
    prop_assert_eq!(&once.lines, &twice.lines);
    for l in &once.lines {
        prop_assert!(allowed_line(l), "not allowed: {l}");
    }
    Ok(())
}

#[test]
fn every_corpus_model_parses_and_reemits() {
    for text in corpus() {
        // Model-card files (diodes, transistors) are not components: refused in words.
        if !text.to_ascii_lowercase().contains(".subckt") {
            assert_eq!(parse(&text).unwrap_err().to_string(), "no .subckt in the model");
            continue;
        }
        let deck = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert!(!deck.subckts.is_empty());
        let emitted = emit_all(&text);
        assert_eq!(emitted.len(), deck.subckts.len(), "{text}");
        for lines in emitted {
            proptest::test_runner::TestRunner::default().run(&Just(()), |_| check_emitted(&lines)).unwrap();
        }
    }
}

const TOKENS: [&str; 48] = [
    ".subckt", ".ends", ".model", ".param", ".func", ".include", ".control", ".end", "params:", "poly(2)", "value=",
    "v=", "i=", "{", "}", "(", ")", "=", ",", "'", "+", "*", ";", "$ ", "r1", "c1", "e1", "g1", "f1", "h1", "b1", "x1",
    "d1", "q1", "v1", "k1", "a", "b", "0", "n1", "1k", "1meg", "2.5", "-1", "v(a,b)", "i(v1)", "d", "npn",
];

fn token_soup() -> impl Strategy<Value = String> {
    prop::collection::vec((0..TOKENS.len(), prop::bool::weighted(0.15)), 0..80).prop_map(|ts| {
        let mut s = String::new();
        for (t, newline) in ts {
            s.push_str(TOKENS[t]);
            s.push(if newline { '\n' } else { ' ' });
        }
        s
    })
}

/// A corpus file with random edits: deleted, inserted or duplicated characters and lines.
fn mutated() -> impl Strategy<Value = String> {
    let corpus = corpus();
    (0..corpus.len(), prop::collection::vec((0usize..4, any::<u16>(), any::<u16>(), any::<char>()), 1..12)).prop_map(
        move |(k, edits)| {
            let mut s: Vec<char> = corpus[k].chars().collect();
            for (kind, a, b, c) in edits {
                if s.is_empty() {
                    break;
                }
                let i = a as usize % s.len();
                match kind {
                    0 => {
                        s.remove(i);
                    }
                    1 => s.insert(i, c),
                    2 => {
                        let j = (i + 1 + b as usize % 40).min(s.len());
                        let piece: Vec<char> = s[i..j].to_vec();
                        s.splice(i..i, piece);
                    }
                    _ => {
                        let j = (i + 1 + b as usize % 40).min(s.len());
                        s.drain(i..j);
                    }
                }
            }
            s.into_iter().collect()
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn random_text_never_panics(text in "\\PC{0,400}") {
        for lines in emit_all(&text) {
            check_emitted(&lines)?;
        }
    }

    #[test]
    fn token_soup_never_panics(text in token_soup()) {
        for lines in emit_all(&text) {
            check_emitted(&lines)?;
        }
    }

    #[test]
    fn mutated_models_never_panic(text in mutated()) {
        for lines in emit_all(&text) {
            check_emitted(&lines)?;
        }
    }
}
