//! The Circuit IR: a pure netlist with semantics and no geometry (LLD §3).
//! The only stored coordinate is `pinned`, which only a user drag can set.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::units::Quantity;

pub type PartId = String;
pub type RefDes = String;
pub type NetId = String;
pub type BlockId = String;

pub const SCHEMA_VERSION: u16 = 1;
pub const GND: &str = "GND";

/// Hard limits (LLD §1).
pub const MAX_PARTS: usize = 300;
pub const MAX_BLOCKS: usize = 8;
pub const MAX_NETS: usize = 400;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Circuit {
    pub schema_version: u16,
    pub registry_version: String,
    /// +1 per applied op (narration excluded).
    pub rev: u64,
    pub parts: IndexMap<RefDes, PartInstance>,
    pub nets: IndexMap<NetId, Net>,
    pub blocks: IndexMap<BlockId, Block>,
    pub analyses: Vec<Analysis>,
    /// Kept sorted and de-duplicated so the order never depends on edit history.
    pub hints: Vec<LayoutHint>,
}

impl Circuit {
    pub fn new(registry_version: impl Into<String>) -> Circuit {
        Circuit {
            schema_version: SCHEMA_VERSION,
            registry_version: registry_version.into(),
            rev: 0,
            parts: IndexMap::new(),
            nets: IndexMap::new(),
            blocks: IndexMap::new(),
            analyses: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// The net a pin is on, if any.
    pub fn net_of(&self, pin: &PinRef) -> Option<&NetId> {
        self.nets.values().find(|n| n.pins.contains(pin)).map(|n| &n.id)
    }

    /// Map of every connected pin to its net, built in one pass.
    pub fn pin_index(&self) -> BTreeMap<&PinRef, &NetId> {
        let mut idx = BTreeMap::new();
        for net in self.nets.values() {
            for p in &net.pins {
                idx.insert(p, &net.id);
            }
        }
        idx
    }

    /// Part refdes in deterministic natural order (C2 < C10 < R1).
    pub fn sorted_refdes(&self) -> Vec<&RefDes> {
        let mut v: Vec<&RefDes> = self.parts.keys().collect();
        v.sort_by_key(|r| refdes_sort_key(r));
        v
    }
}

/// Natural sort key for a refdes: prefix letters, then the number.
pub fn refdes_sort_key(r: &str) -> (String, u64, String) {
    let split = r.find(|c: char| c.is_ascii_digit()).unwrap_or(r.len());
    let (prefix, digits) = r.split_at(split);
    (prefix.to_string(), digits.parse().unwrap_or(u64::MAX), r.to_string())
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct PartInstance {
    pub refdes: RefDes,
    pub part: PartId,
    /// Every param declared by the registry part, keyed by name ("resistance" -> 10kΩ).
    pub params: BTreeMap<String, Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Placement>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Llm { job_id: String },
    User,
    Template { id: String },
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    /// Degrees: 0, 90, 180 or 270.
    #[serde(default)]
    pub rot: u16,
    #[serde(default)]
    pub flip: bool,
}

/// A part pin, written `"R3.1"` or `"U1.OUT_A"` on the wire.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PinRef {
    pub refdes: RefDes,
    pub pin: String,
}

impl PinRef {
    pub fn new(refdes: impl Into<String>, pin: impl Into<String>) -> PinRef {
        PinRef { refdes: refdes.into(), pin: pin.into() }
    }
}

impl fmt::Display for PinRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.refdes, self.pin)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid pin reference \"{0}\" (expected REFDES.PIN)")]
pub struct PinRefParseError(pub String);

impl FromStr for PinRef {
    type Err = PinRefParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (r, p) = s.split_once('.').ok_or_else(|| PinRefParseError(s.to_string()))?;
        let ok = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !ok(r) || !ok(p) {
            return Err(PinRefParseError(s.to_string()));
        }
        Ok(PinRef::new(r, p))
    }
}

impl Serialize for PinRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PinRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = Cow::<str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for PinRef {
    fn schema_name() -> Cow<'static, str> {
        "PinRef".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[A-Za-z0-9_]+\\.[A-Za-z0-9_]+$",
            "description": "Part pin as REFDES.PIN, e.g. \"R3.1\" or \"U1.OUT_A\"."
        })
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Net {
    pub id: NetId,
    pub pins: BTreeSet<PinRef>,
    pub kind: NetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetKind {
    Signal,
    Power { volts: f64 },
    Ground,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub role: BlockRole,
    pub title: String,
    pub spec: BTreeMap<String, SpecTarget>,
    pub ports: Vec<BlockPort>,
    pub status: BlockStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BlockRole {
    Supply,
    Source,
    Bias,
    Amplifier,
    Buffer,
    Filter,
    Oscillator,
    Comparator,
    Load,
    Other,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    Planned,
    Composing,
    Verified,
    Committed,
    Failed,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct SpecTarget {
    pub target: f64,
    pub tol_pct: f64,
}

/// A named interface of a block, bound to the net that carries it (checked by ERC009).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct BlockPort {
    pub name: String,
    pub direction: PortDirection,
    pub net: NetId,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    Input,
    Output,
    Bidir,
    PowerPos,
    PowerNeg,
    Ground,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Analysis {
    Op,
    Dc { source: RefDes, start: f64, stop: f64, step: f64 },
    Ac { points_per_decade: u32, f_start: f64, f_stop: f64 },
    Tran { t_step: f64, t_stop: f64 },
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Right,
    Down,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutHint {
    Flow { dir: Direction },
    Near { a: RefDes, b: RefDes },
    Group { block: BlockId },
}

impl LayoutHint {
    pub fn mentions_part(&self, refdes: &str) -> bool {
        matches!(self, LayoutHint::Near { a, b } if a == refdes || b == refdes)
    }
    pub fn mentions_block(&self, block: &str) -> bool {
        matches!(self, LayoutHint::Group { block: b } if b == block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinref_round_trip() {
        let p: PinRef = "U1.OUT_A".parse().unwrap();
        assert_eq!(p, PinRef::new("U1", "OUT_A"));
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"U1.OUT_A\"");
        assert!("R3".parse::<PinRef>().is_err());
        assert!("R3.".parse::<PinRef>().is_err());
        assert!("R 3.1".parse::<PinRef>().is_err());
    }

    #[test]
    fn natural_refdes_order() {
        let mut v = vec!["R10", "C2", "R2", "C10", "U1"];
        v.sort_by_key(|r| refdes_sort_key(r));
        assert_eq!(v, ["C2", "C10", "R2", "R10", "U1"]);
    }
}
