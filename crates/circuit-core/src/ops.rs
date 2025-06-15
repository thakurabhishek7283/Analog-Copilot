//! Op protocol (LLD §4). Every change to a circuit, by the LLM or the user, is one op in the
//! same envelope; the circuit is the fold of its op log.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, OpError};
use crate::ir::{
    Analysis, BlockId, BlockPort, BlockRole, BlockStatus, LayoutHint, NetId, NetKind, Origin, PartId, PinRef,
    Placement, RefDes, SpecTarget,
};

/// Current envelope version. The server accepts `v` and `v - 1`.
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpEnvelope {
    pub v: u16,
    pub seq: u64,
    #[serde(flatten)]
    pub op: Op,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
    pub base_rev: u64,
}

/// Written by hand as `allOf: [Op, {envelope fields}]`: the derived schema for a flattened
/// tagged enum puts `properties` and `oneOf` side by side, which TS/Pydantic codegen mishandles.
impl JsonSchema for OpEnvelope {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OpEnvelope".into()
    }

    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "One op in its envelope (LLD §4): `{v, seq, op, author, job?, block?, base_rev, body}`.",
            "allOf": [
                g.subschema_for::<Op>(),
                {
                    "type": "object",
                    "properties": {
                        "v": g.subschema_for::<u16>(),
                        "seq": g.subschema_for::<u64>(),
                        "author": g.subschema_for::<Author>(),
                        "job": g.subschema_for::<Option<String>>(),
                        "block": g.subschema_for::<Option<BlockId>>(),
                        "base_rev": g.subschema_for::<u64>(),
                    },
                    "required": ["v", "seq", "author", "base_rev"],
                }
            ]
        })
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Llm,
    User,
    Template,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "op", content = "body")]
pub enum Op {
    #[serde(rename = "block.begin")]
    BlockBegin(BlockBegin),
    #[serde(rename = "block.commit")]
    BlockCommit(BlockRef),
    #[serde(rename = "block.abort")]
    BlockAbort(BlockAbort),
    /// Inverse of `block.begin`.
    #[serde(rename = "block.remove")]
    BlockRemove(BlockRef),
    /// Inverse of `block.commit` / `block.abort`.
    #[serde(rename = "block.set_status")]
    BlockSetStatus(BlockSetStatus),
    #[serde(rename = "part.add")]
    PartAdd(PartAdd),
    #[serde(rename = "part.remove")]
    PartRemove(PartRef),
    #[serde(rename = "part.set_param")]
    PartSetParam(PartSetParam),
    #[serde(rename = "part.swap")]
    PartSwap(PartSwap),
    #[serde(rename = "part.pin")]
    PartPin(PartPin),
    #[serde(rename = "net.connect")]
    NetConnect(NetConnect),
    #[serde(rename = "net.disconnect")]
    NetDisconnect(NetDisconnect),
    #[serde(rename = "net.rename")]
    NetRename(NetRename),
    #[serde(rename = "analysis.set")]
    AnalysisSet(AnalysisSet),
    #[serde(rename = "hint.add")]
    HintAdd(HintBody),
    /// Inverse of `hint.add`.
    #[serde(rename = "hint.remove")]
    HintRemove(HintBody),
    #[serde(rename = "narrate")]
    Narrate(Narrate),
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::BlockBegin(_) => "block.begin",
            Op::BlockCommit(_) => "block.commit",
            Op::BlockAbort(_) => "block.abort",
            Op::BlockRemove(_) => "block.remove",
            Op::BlockSetStatus(_) => "block.set_status",
            Op::PartAdd(_) => "part.add",
            Op::PartRemove(_) => "part.remove",
            Op::PartSetParam(_) => "part.set_param",
            Op::PartSwap(_) => "part.swap",
            Op::PartPin(_) => "part.pin",
            Op::NetConnect(_) => "net.connect",
            Op::NetDisconnect(_) => "net.disconnect",
            Op::NetRename(_) => "net.rename",
            Op::AnalysisSet(_) => "analysis.set",
            Op::HintAdd(_) => "hint.add",
            Op::HintRemove(_) => "hint.remove",
            Op::Narrate(_) => "narrate",
        }
    }

    pub const NAMES: [&'static str; 17] = [
        "block.begin",
        "block.commit",
        "block.abort",
        "block.remove",
        "block.set_status",
        "part.add",
        "part.remove",
        "part.set_param",
        "part.swap",
        "part.pin",
        "net.connect",
        "net.disconnect",
        "net.rename",
        "analysis.set",
        "hint.add",
        "hint.remove",
        "narrate",
    ];
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockBegin {
    pub id: BlockId,
    pub role: BlockRole,
    pub title: String,
    #[serde(default)]
    pub spec: BTreeMap<String, SpecTarget>,
    #[serde(default)]
    pub ports: Vec<BlockPort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockRef {
    pub id: BlockId,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockAbort {
    pub id: BlockId,
    pub reason: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockSetStatus {
    pub id: BlockId,
    pub status: BlockStatus,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartAdd {
    pub refdes: RefDes,
    pub part: PartId,
    /// Values as written ("10k", "4.7u"); unspecified params take the registry default.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
    /// Normally derived from the envelope; set explicitly by inverse ops so undo restores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartRef {
    pub refdes: RefDes,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartSetParam {
    pub refdes: RefDes,
    pub key: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartSwap {
    pub refdes: RefDes,
    pub part: PartId,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartPin {
    pub refdes: RefDes,
    pub placement: Option<Placement>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetConnect {
    pub net: NetId,
    pub pins: Vec<PinRef>,
    /// Applied when the net is created; must match if the net exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NetKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetDisconnect {
    pub net: NetId,
    pub pins: Vec<PinRef>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetRename {
    pub from: NetId,
    pub to: NetId,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisSet {
    pub analyses: Vec<Analysis>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HintBody {
    pub hint: LayoutHint,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Narrate {
    pub refs: Vec<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
}

/// Parse one envelope. An unknown op name is rejected as `unknown_op`, never ignored.
pub fn parse_envelope(json: &str) -> Result<OpEnvelope, OpError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| OpError::new(ErrorCode::SchemaError, e.to_string()))?;
    envelope_from_value(value)
}

pub fn envelope_from_value(value: serde_json::Value) -> Result<OpEnvelope, OpError> {
    match value.get("op").and_then(|o| o.as_str()) {
        Some(name) if !Op::NAMES.contains(&name) => {
            return Err(OpError::new(ErrorCode::UnknownOp, format!("unknown op \"{name}\"")));
        }
        None => return Err(OpError::new(ErrorCode::SchemaError, "missing \"op\"")),
        _ => {}
    }
    serde_json::from_value(value).map_err(|e| OpError::new(ErrorCode::SchemaError, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lld_example() {
        let env = parse_envelope(
            r#"{"v":1,"seq":42,"op":"part.add","author":"llm","job":"j_8f2","block":"b2","base_rev":17,
               "body":{"refdes":"R3","part":"resistor_th","params":{"resistance":"10k"}}}"#,
        )
        .unwrap();
        assert_eq!(env.seq, 42);
        let Op::PartAdd(b) = &env.op else { panic!("wrong op") };
        assert_eq!(b.params["resistance"], "10k");
        let back = serde_json::to_value(&env).unwrap();
        assert_eq!(back["op"], "part.add");
        assert_eq!(back["body"]["refdes"], "R3");
        assert_eq!(parse_envelope(&back.to_string()).unwrap(), env);
    }

    #[test]
    fn parses_net_connect_example() {
        let env = parse_envelope(
            r#"{"v":1,"seq":57,"op":"net.connect","author":"llm","block":"b2","base_rev":31,
               "body":{"net":"N_FB","pins":["U1.OUT_A","U1.INM_A"]}}"#,
        )
        .unwrap();
        let Op::NetConnect(b) = env.op else { panic!("wrong op") };
        assert_eq!(b.pins, vec![PinRef::new("U1", "OUT_A"), PinRef::new("U1", "INM_A")]);
    }

    #[test]
    fn unknown_op_and_bad_body() {
        let e = parse_envelope(r#"{"v":1,"seq":1,"op":"part.explode","author":"user","base_rev":0,"body":{}}"#)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::UnknownOp);
        let e = parse_envelope(
            r#"{"v":1,"seq":1,"op":"part.remove","author":"user","base_rev":0,"body":{"refdes":"R1","x":1}}"#,
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::SchemaError);
    }
}
