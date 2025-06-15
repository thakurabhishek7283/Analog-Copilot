//! Component registry: the only source of pin maps, param schemas and SPICE templates (LLD §12).
//! Loading is pure: callers pass file contents in; this module never touches the filesystem.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ir::PartId;
use crate::units::{Unit, parse_quantity};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Registry {
    pub version: String,
    pub parts: IndexMap<PartId, PartDef>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    R,
    C,
    L,
    D,
    Q,
    U,
    V,
    J,
}

impl Category {
    pub fn letter(self) -> char {
        match self {
            Category::R => 'R',
            Category::C => 'C',
            Category::L => 'L',
            Category::D => 'D',
            Category::Q => 'Q',
            Category::U => 'U',
            Category::V => 'V',
            Category::J => 'J',
        }
    }

    pub fn from_letter(c: char) -> Option<Category> {
        Some(match c {
            'R' => Category::R,
            'C' => Category::C,
            'L' => Category::L,
            'D' => Category::D,
            'Q' => Category::Q,
            'U' => Category::U,
            'V' => Category::V,
            'J' => Category::J,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDef {
    pub id: PartId,
    pub category: Category,
    pub title: String,
    #[serde(default)]
    pub role_tags: Vec<String>,
    /// Units of a multi-unit IC (e.g. `[A, B]` for a dual op-amp). Empty for single-unit parts.
    #[serde(default)]
    pub units: Vec<String>,
    pub pins: Vec<PinDef>,
    /// Every param must declare a default, so instances always carry a full param set.
    #[serde(default)]
    pub params: IndexMap<String, ParamDef>,
    /// Ratings used by ERC, e.g. `v_supply_max`, `i_max`, `p_max`.
    #[serde(default)]
    pub limits: BTreeMap<String, f64>,
    /// For voltage sources: the param holding V(P) − V(N) at DC (used by ERC007).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc_param: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spice: Option<SpiceDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadboard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teach: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hazard: Option<Hazard>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinDef {
    pub name: String,
    pub num: u16,
    #[serde(rename = "type")]
    pub kind: PinType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PinType {
    Input,
    Output,
    Passive,
    PowerPos,
    PowerNeg,
    /// Not connected internally; exempt from ERC001.
    Nc,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParamDef {
    pub unit: Unit,
    pub default: String,
    /// In SI units. YAML sources may write a prefixed number such as `1p` or `100meg`.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "num_or_eng")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "num_or_eng")]
    pub max: Option<f64>,
}

fn num_or_eng<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(f64),
        Str(String),
    }
    match Option::<NumOrStr>::deserialize(d)? {
        None => Ok(None),
        Some(NumOrStr::Num(x)) => Ok(Some(x)),
        Some(NumOrStr::Str(s)) => {
            parse_quantity(&s, Unit::Unitless).map(|q| Some(q.si)).map_err(serde::de::Error::custom)
        }
    }
}

/// SPICE emission template. `line` is for single-unit parts; `unit_line` is emitted once per
/// used unit of a multi-unit part. Placeholders: `{refdes}`, `{unit}`, pin names, param names.
/// In `unit_line`, a unit pin is named without its `_<unit>` suffix (`{OUT}` → `OUT_A`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpiceDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_line: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Hazard {
    Mains,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{source_name}: {message}")]
pub struct RegistryError {
    pub source_name: String,
    pub message: String,
}

impl PartDef {
    pub fn pin(&self, name: &str) -> Option<&PinDef> {
        self.pins.iter().find(|p| p.name == name)
    }

    /// Pins of one unit of a multi-unit part.
    pub fn unit_pins<'a>(&'a self, unit: &'a str) -> impl Iterator<Item = &'a PinDef> + 'a {
        self.pins.iter().filter(move |p| p.unit.as_deref() == Some(unit))
    }

    /// Resolve a `unit_line` placeholder for `unit`: the unit's own pin first, then a shared pin.
    pub fn resolve_unit_pin(&self, unit: &str, base: &str) -> Option<&PinDef> {
        let suffixed = format!("{base}_{unit}");
        self.pins
            .iter()
            .find(|p| p.unit.as_deref() == Some(unit) && p.name == suffixed)
            .or_else(|| self.pins.iter().find(|p| p.unit.is_none() && p.name == base))
    }

    fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !(ident(&self.id) && self.id.bytes().all(|b| !b.is_ascii_uppercase())) {
            errs.push(format!("id \"{}\" must be lowercase [a-z0-9_]", self.id));
        }
        if self.pins.is_empty() {
            errs.push("part has no pins".into());
        }
        let mut names = BTreeSet::new();
        let mut nums = BTreeSet::new();
        for p in &self.pins {
            if !ident(&p.name) {
                errs.push(format!("pin name \"{}\" must be [A-Za-z0-9_]", p.name));
            }
            if !names.insert(p.name.as_str()) {
                errs.push(format!("duplicate pin name {}", p.name));
            }
            if !nums.insert(p.num) {
                errs.push(format!("duplicate pin number {}", p.num));
            }
            match &p.unit {
                Some(u) if !self.units.contains(u) => errs.push(format!("pin {} names unknown unit {u}", p.name)),
                Some(u) if !p.name.ends_with(&format!("_{u}")) => {
                    errs.push(format!("unit pin {} must end with _{u}", p.name))
                }
                _ => {}
            }
        }
        for (name, def) in &self.params {
            if !ident(name) {
                errs.push(format!("param name \"{name}\" must be [A-Za-z0-9_]"));
            }
            if names.contains(name.as_str()) {
                errs.push(format!("param {name} has the same name as a pin"));
            }
            if let (Some(lo), Some(hi)) = (def.min, def.max)
                && lo > hi
            {
                errs.push(format!("param {name}: min > max"));
            }
            match parse_quantity(&def.default, def.unit) {
                Ok(q) => {
                    if def.min.is_some_and(|lo| q.si < lo) || def.max.is_some_and(|hi| q.si > hi) {
                        errs.push(format!("param {name}: default {} outside min/max", def.default));
                    }
                }
                Err(e) => errs.push(format!("param {name}: default: {e}")),
            }
        }
        if self.category == Category::V {
            for n in ["P", "N"] {
                if self.pin(n).is_none() {
                    errs.push(format!("voltage source must have pin {n}"));
                }
            }
            match self.dc_param.as_deref().map(|p| self.params.get(p)) {
                Some(Some(def)) if def.unit == Unit::Volt => {}
                _ => errs.push("voltage source needs dc_param naming a volt param".into()),
            }
        } else if self.dc_param.is_some() {
            errs.push("dc_param is only valid on voltage sources".into());
        }
        if let Some(sp) = &self.spice {
            match (&sp.line, &sp.unit_line) {
                (Some(line), None) => {
                    let known = |n: &str| n == "refdes" || self.pin(n).is_some() || self.params.contains_key(n);
                    errs.extend(check_placeholders(line, known));
                }
                (None, Some(line)) if !self.units.is_empty() => {
                    for unit in &self.units {
                        let known = |n: &str| {
                            n == "refdes"
                                || n == "unit"
                                || self.resolve_unit_pin(unit, n).is_some()
                                || self.params.contains_key(n)
                        };
                        errs.extend(check_placeholders(line, known));
                    }
                }
                (None, Some(_)) => errs.push("unit_line requires units".into()),
                _ => errs.push("spice needs exactly one of line / unit_line".into()),
            }
        }
        errs
    }
}

/// Every `{name}` in a template must be known; braces must balance.
fn check_placeholders(line: &str, known: impl Fn(&str) -> bool) -> Vec<String> {
    let mut errs = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            errs.push(format!("unbalanced brace in \"{line}\""));
            break;
        };
        let name = &rest[open + 1..open + close];
        if !known(name) {
            errs.push(format!("unknown placeholder {{{name}}} in \"{line}\""));
        }
        rest = &rest[open + close + 1..];
    }
    if rest.contains('}') {
        errs.push(format!("unbalanced brace in \"{line}\""));
    }
    errs
}

impl Registry {
    /// Build and validate a registry. Each part is paired with a source name for error messages.
    pub fn new(
        version: impl Into<String>,
        parts: impl IntoIterator<Item = (String, PartDef)>,
    ) -> Result<Registry, Vec<RegistryError>> {
        let mut errs = Vec::new();
        let mut map = IndexMap::new();
        for (source_name, def) in parts {
            for message in def.validate() {
                errs.push(RegistryError { source_name: source_name.clone(), message });
            }
            if map.contains_key(&def.id) {
                errs.push(RegistryError { source_name, message: format!("duplicate part id {}", def.id) });
                continue;
            }
            map.insert(def.id.clone(), def);
        }
        if errs.is_empty() { Ok(Registry { version: version.into(), parts: map }) } else { Err(errs) }
    }

    /// Load the compiled JSON bundle (what the browser and API receive).
    pub fn from_json(json: &str) -> Result<Registry, Vec<RegistryError>> {
        let raw: Registry = serde_json::from_str(json)
            .map_err(|e| vec![RegistryError { source_name: "bundle".into(), message: e.to_string() }])?;
        Registry::new(raw.version, raw.parts.into_values().map(|p| (p.id.clone(), p)))
    }

    /// Load YAML sources, one part per document: `(source_name, yaml_text)`.
    #[cfg(feature = "yaml")]
    pub fn from_yaml_docs<'a>(
        version: impl Into<String>,
        docs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Registry, Vec<RegistryError>> {
        let mut errs = Vec::new();
        let mut parts = Vec::new();
        for (name, text) in docs {
            match serde_norway::from_str::<PartDef>(text) {
                Ok(p) => parts.push((name.to_string(), p)),
                Err(e) => errs.push(RegistryError { source_name: name.to_string(), message: e.to_string() }),
            }
        }
        let built = Registry::new(version, parts);
        match built {
            Ok(r) if errs.is_empty() => Ok(r),
            Ok(_) => Err(errs),
            Err(more) => {
                errs.extend(more);
                Err(errs)
            }
        }
    }

    pub fn part(&self, id: &str) -> Option<&PartDef> {
        self.parts.get(id)
    }
}
