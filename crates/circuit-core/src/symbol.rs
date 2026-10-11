//! Schematic symbols (LLD §10, §12): pin anchors for the layout engine and the drawings that go
//! into the registry's sprite sheet. The registry is the only source of geometry; the IR has none.
//!
//! A symbol source is an SVG whose viewBox is `0 0 W H` on the schematic grid. Each pin anchor is
//! an invisible marker, a direct child of the root: `<circle data-pin="OUT" cx="60" cy="30" r="0"/>`.
//! Anchors sit on the grid and on the symbol's edge; the edge they sit on is the pin's side (the
//! layout engine's port side). A multi-unit part's symbol draws one unit: its unit pins are
//! anchored by their base name (`OUT` for `OUT_A`, as in `unit_line`), shared pins by their name.
//!
//! Drawings may only use plain shapes, and only `none` or `currentColor` as colours, so the app
//! themes every instance from CSS. The sprite sheet is inlined into the page, so anything active
//! (scripts, styles, links, event handlers, foreign content) is rejected, not stripped.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schematic grid: symbol sizes and pin anchors are multiples of this.
pub const GRID: u32 = 10;
/// Flag drawn on every pin of a ground net, and on every pin of a power net (LLD §10).
pub const FLAG_GROUND: &str = "flag_ground";
pub const FLAG_POWER: &str = "flag_power";
/// The single anchor of a flag symbol.
pub const FLAG_PIN: &str = "P";

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolDef {
    /// Size in schematic units (multiples of [`GRID`]).
    pub width: u32,
    pub height: u32,
    /// Pin anchors by pin name (base name for the unit pins of a multi-unit part).
    pub pins: BTreeMap<String, Anchor>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub x: u32,
    pub y: u32,
    /// The edge the anchor sits on; wires leave the symbol in this direction.
    pub side: Side,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl SymbolDef {
    /// Geometry rules, checked for YAML/SVG sources and JSON bundles alike.
    pub(crate) fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || w % GRID != 0 || h % GRID != 0 {
            errs.push(format!("size {w}x{h} must be positive multiples of {GRID}"));
        }
        if self.pins.is_empty() {
            errs.push("symbol has no pin anchors".into());
        }
        for (name, a) in &self.pins {
            match edge_side(a.x, a.y, w, h) {
                Ok(side) if side == a.side => {}
                Ok(side) => errs.push(format!("anchor {name} lies on the {side:?} edge but says {:?}", a.side)),
                Err(e) => errs.push(format!("anchor {name} at ({}, {}): {e}", a.x, a.y)),
            }
            if a.x % GRID != 0 || a.y % GRID != 0 {
                errs.push(format!("anchor {name} at ({}, {}) is off the {GRID}-unit grid", a.x, a.y));
            }
        }
        errs
    }
}

fn edge_side(x: u32, y: u32, w: u32, h: u32) -> Result<Side, &'static str> {
    if x > w || y > h {
        return Err("outside the symbol");
    }
    let on = [(x == 0, Side::Left), (x == w, Side::Right), (y == 0, Side::Top), (y == h, Side::Bottom)];
    let mut sides = on.iter().filter(|(hit, _)| *hit).map(|(_, s)| *s);
    match (sides.next(), sides.next()) {
        (Some(s), None) => Ok(s),
        (Some(_), Some(_)) => Err("on a corner, so its side is ambiguous"),
        _ => Err("not on the symbol's edge"),
    }
}

/// `symbols/<id>.svg` → `<id>` (lowercase `[a-z0-9_]`).
pub fn symbol_id(path: &str) -> Option<&str> {
    let id = path.strip_prefix("symbols/")?.strip_suffix(".svg")?;
    let ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    ok.then_some(id)
}

/// A parsed symbol source: its geometry and its drawing without the anchor markers.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedSymbol {
    pub def: SymbolDef,
    /// Sanitized SVG markup for the inside of the `<symbol>` element.
    pub body: String,
}

/// The sprite sheet the browser inlines once (LLD §10): one `<symbol id="sym-<id>">` per symbol,
/// in the order given. Deterministic, so the bundle is reproducible.
pub fn sprite_sheet<'a>(symbols: impl IntoIterator<Item = (&'a str, &'a ParsedSymbol)>) -> String {
    let mut out = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\">\n");
    for (id, s) in symbols {
        out.push_str(&format!(
            "<symbol id=\"sym-{id}\" viewBox=\"0 0 {} {}\" overflow=\"visible\">{}</symbol>\n",
            s.def.width, s.def.height, s.body
        ));
    }
    out.push_str("</svg>\n");
    out
}

/// A library component's symbol when it has no registry symbol (`symbol: auto`, LLD §17): a box in
/// the registry's own style (the NE555's), inputs on the left, outputs on the right, positive
/// supplies on top, negative supplies and ground below; passive and `nc` pins fill the shorter
/// side. Built here, not in the editor, so geometry still comes only from the core.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AutoSymbol {
    /// Symbol id in the sprite sheet: `lib_<name>`.
    pub id: String,
    pub def: SymbolDef,
    /// SVG markup for the inside of its `<symbol>` (plain shapes, `currentColor`, pin names as text).
    pub body: String,
}

/// Pin spacing on an auto symbol's sides, and the stub between box and anchor.
const AUTO_PITCH: u32 = 2 * GRID;

pub fn auto_symbol(part: &crate::registry::PartDef) -> AutoSymbol {
    use crate::registry::PinType;
    let (mut left, mut right, mut top, mut bottom) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut free = Vec::new();
    for p in &part.pins {
        match p.kind {
            PinType::Input => left.push(p.name.as_str()),
            PinType::Output => right.push(p.name.as_str()),
            PinType::PowerPos => top.push(p.name.as_str()),
            PinType::PowerNeg => bottom.push(p.name.as_str()),
            PinType::Passive | PinType::Nc => free.push(p.name.as_str()),
        }
    }
    for name in free {
        if left.len() <= right.len() { left.push(name) } else { right.push(name) }
    }
    // Pin names are [A-Za-z0-9_] (the registry checks it), so they need no escaping. About 4.5
    // units per character at the 7-unit font.
    let text_w = |names: &[&str]| names.iter().map(|n| n.len() as u32 * 9 / 2).max().unwrap_or(0);
    // Widths are whole multiples of two grid units, so pins centred on top and below sit on the grid.
    let snap = |x: u32| x.div_ceil(2 * GRID) * 2 * GRID;
    let rows = left.len().max(right.len()).max(1) as u32;
    let cols = top.len().max(bottom.len()) as u32;
    let height = 2 * GRID + AUTO_PITCH * (rows + 1);
    let width =
        snap((4 * GRID + AUTO_PITCH * cols).max(6 * GRID).max(2 * GRID + text_w(&left) + text_w(&right) + 3 * GRID));
    let mut pins = BTreeMap::new();
    let mut body =
        format!("<rect x=\"{GRID}\" y=\"{GRID}\" width=\"{}\" height=\"{}\"/>", width - 2 * GRID, height - 2 * GRID);
    let mut stubs = String::new();
    let mut labels = String::new();
    let row_y = |i: usize| GRID + AUTO_PITCH * (i as u32 + 1);
    let col_x = |i: usize, n: usize| width / 2 + AUTO_PITCH * i as u32 - GRID * (n as u32 - 1);
    for (i, name) in left.iter().enumerate() {
        let y = row_y(i);
        pins.insert(name.to_string(), Anchor { x: 0, y, side: Side::Left });
        stubs.push_str(&format!("M0 {y}H{GRID}"));
        labels.push_str(&format!("<text x=\"{}\" y=\"{}.5\">{name}</text>", GRID + 3, y + 2));
    }
    for (i, name) in right.iter().enumerate() {
        let y = row_y(i);
        pins.insert(name.to_string(), Anchor { x: width, y, side: Side::Right });
        stubs.push_str(&format!("M{} {y}H{width}", width - GRID));
        labels.push_str(&format!(
            "<text x=\"{}\" y=\"{}.5\" text-anchor=\"end\">{name}</text>",
            width - GRID - 3,
            y + 2
        ));
    }
    for (i, name) in top.iter().enumerate() {
        let x = col_x(i, top.len());
        pins.insert(name.to_string(), Anchor { x, y: 0, side: Side::Top });
        stubs.push_str(&format!("M{x} 0V{GRID}"));
        labels.push_str(&format!("<text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{name}</text>", GRID + 8));
    }
    for (i, name) in bottom.iter().enumerate() {
        let x = col_x(i, bottom.len());
        pins.insert(name.to_string(), Anchor { x, y: height, side: Side::Bottom });
        stubs.push_str(&format!("M{x} {}V{height}", height - GRID));
        labels.push_str(&format!("<text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{name}</text>", height - GRID - 3));
    }
    body.push_str(&format!("<path d=\"{stubs}\"/>"));
    body.push_str(&format!("<g font-size=\"7\" fill=\"currentColor\" stroke=\"none\">{labels}</g>"));
    AutoSymbol { id: crate::library::subckt_name(&part.id), def: SymbolDef { width, height, pins }, body }
}

#[cfg(feature = "sources")]
pub use parse::parse_svg;

#[cfg(feature = "sources")]
mod parse {
    use super::*;

    const SVG_NS: &str = "http://www.w3.org/2000/svg";
    const ELEMENTS: &[&str] = &["g", "path", "line", "polyline", "polygon", "circle", "ellipse", "rect", "text"];
    const ATTRS: &[&str] = &[
        "d",
        "x",
        "y",
        "x1",
        "y1",
        "x2",
        "y2",
        "cx",
        "cy",
        "r",
        "rx",
        "ry",
        "width",
        "height",
        "points",
        "transform",
        "fill",
        "stroke",
        "stroke-width",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-dasharray",
        "font-size",
        "font-weight",
        "text-anchor",
        "dominant-baseline",
    ];
    const COLOUR_ATTRS: &[&str] = &["fill", "stroke"];

    /// Parse and sanitize one symbol source. Errors list every problem found.
    pub fn parse_svg(svg: &str) -> Result<ParsedSymbol, Vec<String>> {
        let doc = roxmltree::Document::parse(svg).map_err(|e| vec![format!("not valid XML: {e}")])?;
        let root = doc.root_element();
        let mut errs = Vec::new();
        if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
            return Err(vec![format!("root element must be <svg xmlns=\"{SVG_NS}\">")]);
        }
        for a in root.attributes() {
            if a.namespace().is_some() || !matches!(a.name(), "viewBox" | "width" | "height") {
                errs.push(format!("attribute {} is not allowed on <svg>", a.name()));
            }
        }
        let (width, height) = match root.attribute("viewBox").map(view_box) {
            Some(Ok(wh)) => wh,
            Some(Err(e)) => return Err(vec![e]),
            None => return Err(vec!["<svg> needs a viewBox".into()]),
        };

        let mut pins = BTreeMap::new();
        let mut body = String::new();
        for child in root.children() {
            if child.is_element() && child.attribute("data-pin").is_some() {
                match anchor(child, width, height) {
                    Ok((name, a)) => {
                        if pins.insert(name.clone(), a).is_some() {
                            errs.push(format!("duplicate anchor {name}"));
                        }
                    }
                    Err(e) => errs.push(e),
                }
            } else {
                emit(child, &mut body, &mut errs);
            }
        }
        let def = SymbolDef { width, height, pins };
        errs.extend(def.validate());
        if errs.is_empty() { Ok(ParsedSymbol { def, body }) } else { Err(errs) }
    }

    fn view_box(v: &str) -> Result<(u32, u32), String> {
        let nums: Vec<&str> = v.split([' ', ',']).filter(|s| !s.is_empty()).collect();
        match nums.as_slice() {
            ["0", "0", w, h] => match (w.parse(), h.parse()) {
                (Ok(w), Ok(h)) => Ok((w, h)),
                _ => Err(format!("viewBox \"{v}\": width and height must be whole numbers")),
            },
            _ => Err(format!("viewBox \"{v}\" must be \"0 0 W H\"")),
        }
    }

    fn anchor(n: roxmltree::Node, w: u32, h: u32) -> Result<(String, Anchor), String> {
        let name = n.attribute("data-pin").unwrap_or_default().to_string();
        if n.tag_name().name() != "circle" {
            return Err(format!("anchor {name} must be a <circle>"));
        }
        for a in n.attributes() {
            if !matches!(a.name(), "data-pin" | "cx" | "cy" | "r") {
                return Err(format!("anchor {name}: attribute {} is not allowed", a.name()));
            }
        }
        let coord = |k: &str| -> Result<u32, String> {
            n.attribute(k)
                .ok_or_else(|| format!("anchor {name} needs {k}"))?
                .parse()
                .map_err(|_| format!("anchor {name}: {k} must be a whole number"))
        };
        let (x, y) = (coord("cx")?, coord("cy")?);
        let side = edge_side(x, y, w, h).map_err(|e| format!("anchor {name} at ({x}, {y}): {e}"))?;
        Ok((name, Anchor { x, y, side }))
    }

    fn emit(n: roxmltree::Node, out: &mut String, errs: &mut Vec<String>) {
        if n.is_text() {
            if !n.text().unwrap_or_default().trim().is_empty() {
                errs.push(format!("text outside <text>: \"{}\"", n.text().unwrap_or_default().trim()));
            }
            return;
        }
        if !n.is_element() {
            return; // comments, processing instructions
        }
        let name = n.tag_name().name();
        if n.tag_name().namespace() != Some(SVG_NS) || !ELEMENTS.contains(&name) {
            errs.push(format!("element <{name}> is not allowed"));
            return;
        }
        out.push('<');
        out.push_str(name);
        for a in n.attributes() {
            if a.namespace().is_some() || !ATTRS.contains(&a.name()) {
                errs.push(format!("attribute {} is not allowed on <{name}>", a.name()));
                continue;
            }
            if COLOUR_ATTRS.contains(&a.name()) && !matches!(a.value(), "none" | "currentColor") {
                errs.push(format!("{}=\"{}\": use none or currentColor so the app can theme it", a.name(), a.value()));
            }
            out.push_str(&format!(" {}=\"{}\"", a.name(), escape(a.value())));
        }
        if name == "text" {
            out.push('>');
            for c in n.children() {
                match c.text() {
                    Some(t) if c.is_text() => out.push_str(&escape(t.trim())),
                    _ => errs.push("<text> may only contain text".into()),
                }
            }
            out.push_str("</text>");
        } else if n.children().any(|c| c.is_element()) {
            out.push('>');
            for c in n.children() {
                emit(c, out, errs);
            }
            out.push_str(&format!("</{name}>"));
        } else {
            for c in n.children() {
                emit(c, out, errs); // reports stray text
            }
            out.push_str("/>");
        }
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }
}

#[cfg(all(test, feature = "sources"))]
mod tests {
    use super::*;

    fn svg(inner: &str) -> String {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 60 20\">{inner}</svg>")
    }

    fn errs(inner: &str) -> String {
        parse_svg(&svg(inner)).unwrap_err().join("\n")
    }

    const PINS: &str = r#"<circle data-pin="1" cx="0" cy="10" r="0"/><circle data-pin="2" cx="60" cy="10" r="0"/>"#;

    #[test]
    fn parses_anchors_and_strips_markers() {
        let s = parse_svg(&svg(&format!(
            r#"<path d="M0 10H60"/><!-- c --><text x="1" y="2" fill="currentColor"> R </text>{PINS}"#
        )))
        .unwrap();
        assert_eq!((s.def.width, s.def.height), (60, 20));
        assert_eq!(s.def.pins["1"], Anchor { x: 0, y: 10, side: Side::Left });
        assert_eq!(s.def.pins["2"].side, Side::Right);
        assert_eq!(s.body, r#"<path d="M0 10H60"/><text x="1" y="2" fill="currentColor">R</text>"#);
        let sheet = sprite_sheet([("resistor", &s)]);
        assert!(sheet.contains(r#"<symbol id="sym-resistor" viewBox="0 0 60 20" overflow="visible"><path"#));
    }

    #[test]
    fn rejects_bad_anchors() {
        assert!(errs(r#"<circle data-pin="1" cx="30" cy="10"/>"#).contains("not on the symbol's edge"));
        assert!(errs(r#"<circle data-pin="1" cx="0" cy="0"/>"#).contains("corner"));
        assert!(errs(r#"<circle data-pin="1" cx="0" cy="5"/>"#).contains("off the 10-unit grid"));
        assert!(
            errs(r#"<circle data-pin="1" cx="0" cy="10"/><circle data-pin="1" cx="60" cy="10"/>"#)
                .contains("duplicate anchor 1")
        );
        assert!(errs(r#"<rect data-pin="1" x="0" y="10"/>"#).contains("must be a <circle>"));
        assert!(errs("<path d=\"M0 0\"/>").contains("no pin anchors"));
    }

    #[test]
    fn rejects_active_or_unthemeable_content() {
        assert!(errs(&format!("<script>alert(1)</script>{PINS}")).contains("<script> is not allowed"));
        assert!(errs(&format!("<path d=\"M0 0\" onclick=\"x()\"/>{PINS}")).contains("onclick is not allowed"));
        assert!(errs(&format!("<path d=\"M0 0\" style=\"fill:red\"/>{PINS}")).contains("style is not allowed"));
        assert!(errs(&format!("<path d=\"M0 0\" stroke=\"#f00\"/>{PINS}")).contains("currentColor"));
        assert!(errs(&format!("<foreignObject/>{PINS}")).contains("<foreignObject> is not allowed"));
        assert!(errs(&format!("loose text{PINS}")).contains("text outside <text>"));
        let xlink = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:l=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 60 20\"><g l:href=\"#x\"/>{PINS}</svg>"
        );
        assert!(parse_svg(&xlink).unwrap_err().join("\n").contains("href is not allowed"));
    }

    #[test]
    fn rejects_bad_roots() {
        assert!(parse_svg("<svg viewBox=\"0 0 60 20\"/>").unwrap_err()[0].contains("xmlns"));
        assert!(parse_svg("<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap_err()[0].contains("viewBox"));
        let shifted = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"5 0 60 20\"/>";
        assert!(parse_svg(shifted).unwrap_err()[0].contains("0 0 W H"));
        let odd = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 55 20\"><circle data-pin=\"1\" cx=\"0\" cy=\"10\"/></svg>";
        assert!(parse_svg(odd).unwrap_err().join("\n").contains("multiples of 10"));
        assert!(parse_svg("<svg").unwrap_err()[0].contains("not valid XML"));
    }

    #[test]
    fn symbol_ids_come_from_paths() {
        assert_eq!(symbol_id("symbols/opamp.svg"), Some("opamp"));
        assert_eq!(symbol_id("symbols/Opamp.svg"), None);
        assert_eq!(symbol_id("opamp.svg"), None);
        assert_eq!(symbol_id("symbols/../x.svg"), None);
    }
}

#[cfg(test)]
mod auto_tests {
    use super::*;
    use crate::registry::{Category, PartDef, PinDef, PinType};

    fn part(pins: &[(&str, PinType)]) -> PartDef {
        PartDef {
            id: "lib:t".into(),
            category: Category::X,
            prefix: None,
            title: "T".into(),
            role_tags: vec![],
            units: vec![],
            pins: pins
                .iter()
                .enumerate()
                .map(|(i, (n, k))| PinDef {
                    name: n.to_string(),
                    num: i as u16 + 1,
                    kind: *k,
                    unit: None,
                    dc_group: None,
                })
                .collect(),
            params: Default::default(),
            limits: Default::default(),
            dc_param: None,
            spice: None,
            symbol: "auto".into(),
            breadboard: None,
            teach: None,
            hazard: None,
        }
    }

    #[test]
    fn auto_symbols_follow_the_registry_rules() {
        use PinType::*;
        let cases: Vec<Vec<(&str, PinType)>> = vec![
            vec![("IN", Input), ("OUT", Output), ("VCC", PowerPos), ("VEE", PowerNeg)],
            vec![("A", Passive)],
            vec![
                ("IN1", Input),
                ("IN2", Input),
                ("IN3", Input),
                ("OUT", Output),
                ("REF", Passive),
                ("V1", PowerPos),
                ("V2", PowerPos),
                ("G", PowerNeg),
            ],
            vec![("A_VERY_LONG_PIN_NAME", Input), ("ANOTHER_LONG_ONE", Output)],
        ];
        for pins in cases {
            let s = auto_symbol(&part(&pins));
            assert_eq!(s.id, "lib_t");
            assert!(s.def.validate().is_empty(), "{pins:?}: {:?}", s.def.validate());
            assert_eq!(s.def.pins.len(), pins.len(), "every pin has an anchor");
            assert!(!s.body.contains("data-pin"));
        }
        let s = auto_symbol(&part(&[("IN", Input), ("OUT", Output), ("VCC", PowerPos), ("VEE", PowerNeg)]));
        assert_eq!(s.def.pins["IN"].side, Side::Left);
        assert_eq!(s.def.pins["OUT"].side, Side::Right);
        assert_eq!(s.def.pins["VCC"].side, Side::Top);
        assert_eq!(s.def.pins["VEE"].side, Side::Bottom);
    }
}
