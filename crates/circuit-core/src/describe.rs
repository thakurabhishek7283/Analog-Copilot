//! Compact netlist text for LLM prompts (LLD §6, prompt item 4): about 4× fewer tokens than JSON.
//!
//! ```text
//! U1 opamp_tl072 [b2] OUT_A:N_OUT INM_A:N_OUT INP_A:N_B VEE:VEE VCC:VCC INP_B:- INM_B:- OUT_B:-
//! R1 resistor_th [b2] resistance=10kΩ 1:N_IN 2:N_A
//! NETS GND:ground VCC:power(12V)
//! ```

use crate::ir::{Circuit, NetKind};
use crate::registry::Registry;
use crate::units::{Unit, format_eng};

pub fn circuit_text(c: &Circuit, reg: &Registry) -> String {
    let pin_net = c.pin_index();
    let mut out = String::new();
    for refdes in c.sorted_refdes() {
        let inst = &c.parts[refdes];
        out.push_str(&format!("{refdes} {}", inst.part));
        if let Some(b) = &inst.block {
            out.push_str(&format!(" [{b}]"));
        }
        for (k, q) in &inst.params {
            out.push_str(&format!(" {k}={}", q.display));
        }
        if let Some(def) = reg.part(&inst.part) {
            for p in &def.pins {
                let net = pin_net.get(&crate::ir::PinRef::new(refdes, &p.name)).map(|n| n.as_str()).unwrap_or("-");
                out.push_str(&format!(" {}:{net}", p.name));
            }
        }
        out.push('\n');
    }
    let special: Vec<String> = c
        .nets
        .values()
        .filter_map(|n| match n.kind {
            NetKind::Signal => None,
            NetKind::Ground => Some(format!("{}:ground", n.id)),
            NetKind::Power { volts } => Some(format!("{}:power({})", n.id, format_eng(volts, Unit::Volt))),
        })
        .collect();
    if !special.is_empty() {
        out.push_str(&format!("NETS {}\n", special.join(" ")));
    }
    out
}
