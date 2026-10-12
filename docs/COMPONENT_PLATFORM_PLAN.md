# Component platform: user-defined building blocks, a large library, complex prompts

Status: approved (user, 2026-10-11), every recommendation taken (section 11). Its decisions are summarised in
[LLD.md](LLD.md) §17 and its parts tracked in [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) as Phase 5.

## 1. Strategy in one paragraph

Make **component** the single, recursive building unit. A resistor, a Sallen-Key filter, a black box you
describe with equations, an imported vendor model and (later) a CPU are all components: an **interface**
(typed pins), a **model** (what it does), and a **contract** (the laws and limits it must obey, checked by
simulation). circuit-core parses, compiles, checks and verifies every kind of component the same way, in the
browser and on the server. Users create components from a selection of parts, from equations, or from a SPICE
model, and save them to their library. The AI composes circuits from components. When a component is
missing, the AI writes one through the same verify loop instead of refusing. Everything stays deterministic:
the LLM still never emits geometry, SPICE text or arithmetic results, and every IR change still goes
through `apply()`.

## 2. Why the current design cannot stretch

Your three-band analyser prompt fails at several points, each with a structural cause.

| What happens | Where | Root cause |
| --- | --- | --- |
| Prompt refused (it is ~4,800 characters) | `routers/projects.py` `MAX_PROMPT = 2000`, `GeneratePanel.tsx` `maxLength` | A fixed cap sized for one-line requests; nothing turns a long spec into structure |
| Needs ~11 blocks | `ir.rs` `MAX_BLOCKS = 8`, plan schema `max_length=8` | Flat blocks; no subsystems, so every stage counts against one small limit |
| "LM358 on 5 V" cannot be met | Every op-amp template is a TL072 on ±12 V rails (min 9 V); only the comparator and Schmitt use the LM358 | Templates hard-wire one part and one supply style. There is no notion of "any op-amp" |
| Single-supply biasing (2.5 V reference) impossible | Filters and amplifiers tie their reference points to `gnd` | No **signal reference** port. A gain stage tied to GND on one supply multiplies the 2.5 V bias and saturates |
| "Adjustable gain", "test points" | 15 registry parts, no potentiometer or test point | Small library. `Category` is a closed enum (R C L D Q U V J), so new kinds of part need core changes |
| Any block the library lacks | Planner lists it in `uncovered`, job ends `unsupported_request` | Blocks only come from templates. A new template needs a Rust solver (19 hard-coded in `template/solvers.rs`), so a new block means a core release |
| 100 Hz / 1 kHz / 5 kHz comparison | One transient, one source frequency | No notion of a **test plan** (the same circuit run under several stimuli) |
| BOM, breadboard estimate | Nothing | No package data in the registry |

A further limit applies at the far end of your goal (LED up to CPU). The IR is flat: inserting a block copies
its parts in, so 8-bit logic at transistor level would be thousands of parts against a 300-part limit. ngspice is
also built with `--disable-xspice` (native and WASM), so there are no event-driven digital models.

## 3. The decisions that are hard to change later

These touch the IR, the op protocol, the registry format or stored data. Once projects and op logs exist in
production, each would need a migration. They should be settled before the public beta (Phase 4).
**Recommendation: do the foundation (parts P5.0 to P5.2 below) before Phase 4's beta launch.** Phase 4's plan
was proposed but not started, so nothing is lost by re-ordering.

### D1. Hierarchy: true component instances, with definitions embedded in the project

**Decision.** The IR gets a project **library** of component definitions, and a part instance may name a
library definition as well as a registry part. The compiler emits each definition once as a `.subckt`, and
each instance as an `X` line. Definitions may nest (a DAG, depth ≤ 8, cycles rejected by `apply()`).

```rust
pub struct Circuit {
    pub schema_version: u16,                    // 2
    pub registry_version: String,
    pub rev: u64,
    pub library: IndexMap<DefId, ComponentDef>, // NEW: definitions this project uses, embedded
    pub parts: IndexMap<RefDes, PartInstance>,  // `part` is "opamp_lm358" (registry) or "lib:vca_1" (library)
    pub nets: IndexMap<NetId, Net>,
    pub blocks: IndexMap<BlockId, Block>,       // unchanged: an expanded, visible group of parts
    pub analyses: Vec<Analysis>,
    pub tests: Vec<TestPlan>,                   // NEW, D8
    pub requirements: Vec<Requirement>,         // NEW, D9
    pub hints: Vec<LayoutHint>,
}
```

**Why embedded rather than referenced.** A project must stay a pure fold of its op log, identical on every
runtime and replayable with no network. If the circuit only referenced `user/vca@3`, opening a project, the
parity tests and the server's fold would all need a library fetch, and a deleted library item would break old
projects. KiCad moved to embedding its symbols in the schematic for the same reason. Definitions are small
(a few KB). "Update from library" is an explicit op, like KiCad's "update symbols from library".

**Two ways to place a definition** (both kept, chosen per insert):

- **Expanded**: today's behaviour. The parts are copied in as a `Block` that students can see and edit.
  This is right for teaching (a Sallen-Key's resistors are the lesson).
- **Instance**: one symbol with the definition's pins. Its inside is reached by "open". This is right for reuse
  and scale (a 3-band analyser's three gain stages, a CPU's 64 flip-flops). The new op `instance.expand`
  turns an instance into an expanded block, so nothing is ever locked away.

**Rejected:** flatten-only (today). It has no reuse, editing a definition does not reach its copies, and the
part limit is reached long before a CPU.

### D2. One concept: component (parts and templates converge)

Today a **part** (registry YAML with a SPICE line) and a **template** (a sub-circuit with targets, a solver and
checks) are different things. A user-defined block needs both natures: pins like a part, an inside and checks
like a template. Merge them into one `ComponentDef`:

```rust
pub struct ComponentDef {
    pub id: DefId,
    pub version: u32,
    pub hash: String,                 // sha256 of canonical JSON, computed by the core, never sent by a client
    pub title: String,
    pub teach: Option<String>,
    pub refdes_prefix: String,        // "U", "X", "TP", "RV", "SW", "M" ...  (D6)
    pub pins: Vec<PinDef>,            // + domain, dc group, drive (D6)
    pub params: IndexMap<String, ParamDef>, // the knobs a user turns (gain, fc)
    pub model: Model,
    pub contract: Contract,           // limits + checks + bench (D10)
    pub symbol: SymbolChoice,         // Auto (generated box) | Registry { id, pin_map }
    pub package: Option<PackageRef>,  // D14
    pub provenance: Provenance,       // Builtin | User | Llm { job } | Import { source, licence }
}

pub enum Model {
    Primitive(SpiceDef),                       // today's registry parts (R, C, vendor .lib subckt)
    Composite { parts, nets, values: Values }, // today's templates, generalised (D5)
    Behavioral { laws: Vec<Law> },             // black box from equations (D3, D4)
    SpiceImport { subckt: SanitizedSubckt },   // a vendor model (D3)
}
```

The registry's existing `parts/*.yaml` and `templates/*.yaml` stay valid as sources. The loader turns them into
`ComponentDef`s, and the 19 Rust solvers stay as named built-in solvers. Nothing already verified is rewritten.
A Rust test asserts that all 20 templates produce byte-identical ops and netlist hashes before and after
(the same technique Phase 2 used for `trial_block`).

### D3. Model kinds for "black box with deterministic behaviour bounded by laws"

| Kind | What the user writes | Compiles to | Use |
| --- | --- | --- | --- |
| Composite | Parts + nets (+ value rules, D5) | `.subckt` of primitives | Any block built from existing parts |
| Behavioral | Laws in the core's expression language (D4) | `.subckt` of B/E/G sources, R, C | Idealised blocks: VCA, ideal op-amp, sensor, LDO, PLL phase detector, logic gate |
| SPICE import | A vendor `.subckt` + pin map | Re-emitted from a parsed, allow-listed AST | Real ICs whose makers publish models |
| Primitive | (registry authors only) | One SPICE line | R, C, L, D, Q, M, sources |

Behavioral laws cover four shapes, each realised without ngspice features that differ between builds:

1. **Static relations**: `V(out) = clamp(V(ref) + gain*(V(inp) - V(inm)), V(vee) + 0.02, V(vcc) - 1.5)` → a B-source.
   The compiler emits hard `min`/`max` limits and bounds every limited *state* as well as the output (a
   conductance pulls the state back when it leaves its limits). Without that, the state winds up: in the
   P5.0 spike a law-written op-amp's Schmitt trigger switched at 4.34 V instead of 2.83 V. Smooth
   (log-sum-exp) corners were 5.6× slower and biased the clamp by 27 mV, so they are not the default.
2. **Linear dynamics**: `V(out) = tf(V(in), num=[1], den=[1/(2*pi*fc), 1])` → a controllable-canonical
   state-space realisation (G-sources into unit capacitors). This avoids relying on `ddt()` or XSPICE `s_xfer`.
3. **Port physics**: `R(in, ref) = 1Meg`, `Rout(out) = 50`, `I(vcc) = 0.7m` → real elements, so loading and
   supply current are physical, not ideal.
4. **Envelope (the "laws you describe" that bound it)**: `require V(vcc) - V(vee) in [3, 32]`,
   `require P(total) < 0.5`, `require I(out) < 20m`. These become ERC rules (static) and post-simulation checks
   on every run, and the black box's tutor context ("this block is idealised; it promises …").

**SPICE import** goes through a parser in circuit-core that only accepts R C L D Q M J K E F G H B V I X elements
and `.subckt/.ends/.model/.param/.func`, and rejects `.control`, `.include`, `.lib`, `shell`, `exec`, file names
and anything over 256 KB. The deck is re-emitted from the AST, so no user text reaches the simulator verbatim.
This keeps LLD §14's "no SPICE injection".

**Rejected for now:** Verilog-A via OSDI/OpenVAF. It is the industry format for compact models, but it compiles to
native shared objects, which the browser's ngspice-WASM cannot load, so browser/server parity would break.
Revisit later for server-only analyses.

### D4. One expression language, in circuit-core, with units

The core needs one small expression language for laws (D3), value rules (D5), derived params, and requirement
targets. It has a hand-written parser and evaluator in Rust, used by both bindings.

- Grammar: numbers with SI prefixes (`4.7k`), params, `V(a)`, `V(a,b)`, `I(pin)`, `P(...)`, `time`, `+ − × ÷ ^`,
  comparison, `if(c, a, b)`, and an allow-listed function set (`exp ln log10 sqrt abs min max clamp tanh sin cos
  pi tf table`).
- **Dimensional analysis**: every term carries a unit, so `V(in) + I(out)` is an error with a plain message.
  This is the cheapest guard against nonsense laws, from a user or a model.
- Emission: the compiler writes SPICE expression syntax from the AST, never from the user's text.
- Determinism: the evaluator keeps the existing rules (`+ − × ÷ √` exact; transcendental results rounded to
  4 significant digits where they decide a part value; ties within 1e-9 keep the first candidate).

This reverses LLD §12's "rejected: a formula language in the YAML". That rejection argued against a *second*
expression language next to Rust solvers, and against formulas doing the E-series search. Here it is the only
user-facing expression language, and the search stays in Rust (D5). The LLD note should be updated to say so.

### D5. Value rules: forward formulas plus a generic E-series search

A user-made parameterised block ("gain stage, gain 1 to 10") must solve for values without a new Rust solver.
The author writes **forward estimates** (values → spec), which are easier to get right than inverse
formulas. The core searches the series for values that best meet the targets:

```yaml
params:   {gain: {unit: unitless, min: 1, max: 10, default: "2"}}
values:
  R1: {resistance: 10k}                          # fixed
  R2: {resistance: "R1*(gain - 1)", series: E24} # derived (optional inverse rule)
  C1: {capacitance: search(E12, 1n, 10u)}        # free: searched
estimates:
  gain: "1 + R2/R1"                              # forward model the search scores
checks:
  - {name: gain, kind: ac_gain, in: in, out: out, tol_pct: 3}
```

The search enumerates free values over their series (bounded at about 10⁵ combinations, or it is an
authoring error), computes derived values, and scores the summed squared relative error of each estimate
against its target, with the existing deterministic tie-break. Simulation then checks the result
(characterisation, D10), so a wrong estimate is caught as a failed check.

### D6. Pin semantics replace category guesses; refdes prefixes open up

ERC today infers behaviour from the part's category (`dc_pins`: R/L/V/D/Q conduct at DC, C/J do not, U conducts
except inputs). That cannot work for a user's black box or a nested composite. Each pin gains:

- `domain`: `electrical` | `logic` (digital later, D11; declaring it now keeps the schema stable);
- `dc_group`: pins joined by a DC path inside the component (for ERC003). Built-ins derive it from their
  category as today. Composites compute it from their own netlist, recursively, once per definition hash.
  Behavioral components derive it from their port physics;
- `drive`: `none` | `weak` | `strong` (ERC006 output conflict), plus the existing `type`.

ERC then runs on the instance's **summary**, so a definition is checked once, not once per instance. That is
what lets a CPU-sized design be checked in under a millisecond.

`Category` stops being a closed enum. A component declares its `refdes_prefix` from an allow-listed set (R C L
D Q M J U X V I TP RV SW K F Y LED BT …), and invariant 4 checks the prefix against the definition, not a
hard-coded table.

### D7. Signal reference port and part slots (single-supply, any op-amp)

- Every op-amp-based template gains a `ref` port: the node its signal is measured against. Points now wired to
  `gnd` for signal purposes (MFB's non-inverting input, a gain leg's bottom resistor, Sallen-Key's C2) go to
  `ref`. `ref` defaults to `GND` (dual supply, today's circuits unchanged) or binds to a mid-rail net
  (`VREF_2V5`) on a single supply. AC checks are already relative. `dc_level` checks become relative to `ref`.
- Template parts become **slots** with an interface, not one fixed part:
  `U1: {interface: opamp_dual, default: opamp_tl072}`. Any registry part, or library component, that implements
  `opamp_dual` (same pin names) fits. Rail ranges come from the chosen part's limits, not the template, so an
  LM358 slot accepts 3 to 32 V single supply. The existing `part.swap` (pins remapped by name) already does
  the runtime half of this.
- Headroom becomes checkable: a new check kind `tran_headroom` (the output stays at least X V inside the part's
  output swing over the run), so "avoid saturation and clipping" is measured, not hoped for.

### D8. Test plans in the IR

"Run at 100 Hz, 1 kHz and 5 kHz at the same amplitude, then sweep 10 Hz to 20 kHz" is a **test plan**:
named scenarios, each with parameter overrides and analyses, plus measurements on any nets (not only block
ports).

```rust
pub struct TestPlan {
    pub id: String,                                  // "freq_compare"
    pub scenarios: Vec<Scenario>,                    // {name, overrides: {"V2.frequency": "1k"}, analyses}
    pub probes: Vec<Probe>,                          // {net or test point, measure: amplitude | dc | ac_db ...}
}
```

Overrides are applied by the core to a scratch copy, so each scenario has its own netlist hash and cache entry.
ngspice has no `.step`, so N scenarios are N runs. The scope shows a comparison table and overlaid traces. The
generator, the tutor's "try it" and the user all set test plans through `test.set`, so they are undoable and
saved like analyses.

### D9. Requirements as first-class data, traced to evidence

A long prompt is first turned into a **design brief**: numbered requirements, each with a kind and, where
measurable, a target the core can check:

```text
R1  supply      single 5 V from a 7805                    → rail V5 = 5 V ±4% (dc_level)
R4  filter      channel A low-pass, fc ≈ 200 Hz            → b4.fc_hz 200 Hz ±10%
R9  constraint  op-amps are LM358                          → structural: every opamp slot = opamp_lm358
R12 deliverable labelled test point per channel output    → structural: TP on CH_A_OUT, CH_B_OUT, CH_C_OUT
R15 test        compare 100 Hz / 1 kHz / 5 kHz             → test plan freq_compare exists and ran
```

Each requirement ends as **met** (with the check, measurement or structural fact that proves it),
**missing**, or **unverifiable** (shown, never counted as met). Requirements are fixed
*before* the build and checked after it, against the assembled circuit (section 7, step 7), never only
against each block's own bench. The user can edit the brief before generation starts.

### D10. Contracts, characterisation and trust levels

Every non-built-in component carries a contract (limits, checks, a test bench in the existing `verify` shape)
and gets a **characterisation report**: its checks run at generated verification points (the existing
`verify_points`, generalised to params), in WASM and on the server, with identical results. The report is
derived data keyed by `(definition hash, registry version, ngspice version)`, like the simulation cache. It is
never stored as a claim inside the IR, and the server recomputes it rather than trusting a client.

| Trust level | Meaning | Who may use it |
| --- | --- | --- |
| Verified | Built-in, CI-verified at release | Everyone, the generator by default |
| Characterised | Passed its own contract in our simulator | Its owner. The generator, when the owner allows it |
| Draft | Not yet characterised, or a check fails | Its owner, in the editor only, with a badge |

### D11. Digital domain: declare now, simulate later

Pins get `domain: logic` now (D6), so a later digital track is additive. For simulation there are three
options, to be settled by a spike, not now:

1. Logic gates as behavioral analog components (works today, fine up to about 50 gates);
2. ngspice XSPICE digital (event-driven, mixed-signal). **Chosen (P5.0 spike):** the WASM build links the
   code models statically (+86 KB gzip, 3.5% slower, all 112 analog decks bit-identical), and a counter,
   adder and DAC deck matched the Windows engine exactly. The Linux native build with XSPICE is still to do;
3. A logic simulator in circuit-core for purely digital sub-designs (largest effort; not needed).

A CPU is reachable only through hierarchy plus option 2 or 3, as a few hundred gate and flip-flop instances.
A transistor-level CPU in the browser is not a goal.

### D12. Limits

- Blocks: 8 → 32 per level. A subsystem (an instance of a composite) counts as one at its level.
- Parts: 300 per level. A separate **leaf-device budget** per simulation: about 1,000 for live re-simulation in
  the browser (measured in P5.0: 0.21 ms per device for OP plus a 1,000-step transient), about 8,000 on the
  server within its 2 s limit. Above the budget the editor switches from auto-run to a Run button.
- Prompt: 2,000 → 16,000 characters (about 4k tokens). The brief (D9), not the raw text, feeds planning.

### D13. Where components live

| Store | Contents | Versioning |
| --- | --- | --- |
| Registry bundle (CDN) | Built-in parts and templates, CI-verified | Immutable per registry version (unchanged) |
| User library (Postgres `components`) | A user's components, all versions, characterisation reports, embeddings | `(id, version)`, content hash |
| Project library (`Circuit.library`) | Copies of the definitions this project uses | Pinned by hash, updated by an explicit op |

New ops: `lib.add {def}` (validated by `apply()`: parse, DAG, units, hash), `lib.update {id, def, pin_map?}`
(inverse: the old definition; instances re-validated, pins remapped like `part.swap`), `lib.remove {id}` (only
when unused), `instance.expand {refdes}`, `test.set`, `req.set`, and `block.set_spec`. The last one is the
"op that edits a spec" the Phase 3 dry run found missing for a deliberate retune. The sim runner needs no change:
definitions are emitted inline in the netlist (no new `.include`s), so the hash still covers everything it runs.

### D14. Packages in the registry

Physical facts (package, pin order on the package, lead pitch, part number, unit packing) go into the
component definition (`package: {kind: dip8, ...}`), not into application code. The BOM, breadboard
placement and "two LM358 halves share one DIP-8" all read it. Footprints never live in application code, and
the model never chooses board positions ("registry is the only source", "the LLM never emits geometry"):
breadboard placement, when it comes (LLD v2), is a deterministic placer in circuit-core that reads packages
from the registry.

### D15. Schema and migration policy

IR `schema_version` 1 → 2, op envelope `v` 1 → 2. The core ships `migrate_v1(Circuit) -> Circuit` and
reads v1 envelopes, so existing projects, cassettes and goldens still fold. The parity CI's random op logs
gain the new ops. Since nothing is public yet, this is the cheapest moment for the change. After the beta,
every saved project constrains the format.

## 4. How it fits the existing architecture

| Layer | Change | Rule kept |
| --- | --- | --- |
| `circuit-core` | `ComponentDef`, library ops, expression language, hierarchical compile, ERC on summaries, generic solver, SPICE import parser, test plans, requirements, BOM, characterisation points | One implementation, both bindings, no I/O |
| Registry | Loader emits `ComponentDef`s. New fields `interface`, `package`, `domain`, `dc_group`. Op-amp macro-model **generator** (below) | Bundle immutable per version |
| `sim_runner` | Unchanged protocol. Characterisation is N bench runs, cached per hash | Sandboxed, no includes outside the registry |
| API | `components` table and `/v1/components` (CRUD, versions, characterise as a job over SSE). Generation v2 (section 7) | Every IR change through `apply()` |
| Web | Library panel, component editor, generated symbols, hierarchical navigation, test plan UI, BOM view | Generated code never hand-edited |
| Tutor | Context lists a component's contract and laws; drill-in for instances | Refs and grounding gates unchanged |

**Part generators** are how the library grows to hundreds of real parts without hand-writing models. The
existing `lm358.lib` is already a hand-written behavioral macro-model (one pole, clamps, supply current).
A generator in circuit-core builds that kind of model from datasheet numbers: op-amps from GBW, slew rate,
offset, bias current, input common-mode range, output swing and quiescent current. Similar generators cover
comparators, LDOs and voltage references. One reviewed generator plus a table of datasheet values gives
LM358, TL072, NE5532, MCP6002, OPA2134 and so on, each characterised automatically. Where a vendor model
exists and its licence allows redistribution, SPICE import is the higher-fidelity option, with the licence
recorded per model (LLD §12 step 4).

## 5. Creating components (the user's side)

Four ways in, all ending in the same validate → characterise → save flow:

1. **From a selection.** Select parts → *Make component*. The core proposes ports (nets that cross the selection
   boundary, with directions from pin types) and the user names them. Part values can be promoted to params
   (`R2 = R1*(gain-1)`). Checks are picked from the closed set on the new ports, and the bench is generated from
   port directions. The selection can be replaced by one instance of the new component.
2. **From laws (black box).** Ports (name, direction, domain), laws, envelope, params. The expression editor shows
   the core's parse and unit errors as you type. *Try it* drops the component into a generated bench with a
   scope.
3. **From a SPICE model.** Paste a `.subckt`, map its pins, pick a symbol. The import parser reports anything it
   refused.
4. **Ask the AI.** "Make a voltage-controlled amplifier with gain 0 to 10 set by a 0 to 5 V control." The model
   writes a definition in the structured format (laws as expressions, never SPICE). The core validates it, the
   repair loop fixes errors as in `trial_block`, the user reviews, and characterisation decides.

The library panel has three tabs: *Built-in*, *My components*, *This project*. Each item shows a trust badge
(D10), a generated or chosen symbol, and its characterisation results.

## 6. Growing the built-in library

| Group | Now | Target | Approach |
| --- | --- | --- | --- |
| Passives (R, C, L, potentiometer, trimmer, thermistor, LDR, fuse, crystal, transformer via `K`) | 4 | ~20 | Primitives with params; sensors as behavioral components |
| Sources and test equipment (DC, sine, pulse, PWL, current source, battery, test point, probe) | 2 | ~10 | Primitives; test point = a 1-pin zero-effect part with a label |
| Semiconductors (diodes, Zeners, LEDs by colour, BJTs, MOSFETs, JFETs) | 6 | ~40 | `.model` parameter sets from datasheets |
| Linear ICs (op-amps, comparators, references, regulators, LDOs, 555, analog switches) | 4 | ~50 | Generators (section 4) + licensed vendor models |
| Logic (gates, flip-flops, counters, mux) | 0 | ~20 | D11 track |
| Electromechanical (switch, relay, motor, speaker, buzzer) | 0 | ~10 | Behavioral |
| Block templates | 20 | ~80 | Data-only templates with value rules (D5). All op-amp templates get `ref` + slots (D7). Add: adjustable-gain stage (pot), active rectifier, peak detector, integrator, differentiator, instrumentation amp, Wien and phase-shift oscillators, VCO, PWM, H-bridge, current source, LED driver, envelope detector, notch, state-variable filter, crossover, mixer |

Each new item passes the existing gates (every part simulated, every template at 5 points, native and WASM).
AI-authored components that users characterise and rate become candidates. A maintainer promotes them into
the built-in registry after review.

## 7. Generation v2 for long, complex prompts

```text
prompt (≤16k chars)
  → 1 brief       large model → requirements R1..Rn (D9); shown, editable, confirmed or auto-accepted
  → 2 architect   subsystems (power, reference, input, channel A/B/C, outputs) and the links between them
  → 3 retrieve    per subsystem: templates, parts, user components by role + constraints
                  (single 5 V, LM358) + similarity (pgvector; the registry no longer fits in one prompt)
  → 4 plan        blocks per subsystem, slots filled (opamp = LM358), `ref` bound to VREF,
                  test points, `new_components[]` for gaps
  → 5 synthesise  each gap: author a ComponentDef (composite first, behavioral if no parts can do it),
                  characterise, repair ≤3; on pass it joins the project library, otherwise it is
                  reported with the nearest substitute
  → 6 compose     per block (today's compose/verify/repair/fallback, unchanged)
  → 7 assemble    whole-circuit verification (ERC, simulation, every block's checks measured in place,
                  filters against their own inputs) + headroom + requirement tracing; ≤2 repairs on a
                  scratch copy, committed only when they pass
  → 8 test plan   run scenarios, fill the comparison table
  → 9 deliver     BOM, part count, breadboard estimate, labelled nets, limitations list
```

Steps 1 and 9 are where "explicitly identify the limitation and substitute" lives. The brief records
conflicts it finds before any building (for example "a 7805 needs at least 7.5 V in, so the input is 9 V").
The delivery lists every requirement that is not met, with its reason.

Budgets: about 15k input tokens per circuit today. A 10-block, 5-subsystem job with one synthesised
component is estimated at 60–90k input tokens. Prompt caching keeps most of that cheap. The token gate is
re-based on the new eval set rather than the 4-block one.

## 8. Your prompt, as the platform should build it

This is the acceptance test for generation v2. Values are what the existing solvers give, rounded as shown.
The simulation decides the final numbers.

**Shared power and input**

- 9 V DC → **7805** (0.33 µF in, 0.1 µF out) → `V5`. Limitation stated: a 7805 needs ≥ 7.5 V in.
- **VREF_2V5**: 10 k / 10 k divider from `V5`, buffered by one LM358 half (follower), 10 µF to ground.
- Sine source 0.5 V peak, frequency adjustable, AC-coupled (1 µF) and biased to VREF through 100 k, then an
  LM358 follower → `BUF_IN` (test point `TP_IN`; reference test point `TP_SRC` on the source).
- 100 nF decoupling at each LM358's pin 8, plus one 10 µF bulk capacitor on `V5`.
- Limitation stated: an LM358 on 5 V swings to about 3.5 V, so a 2.5 V bias leaves about 1 V of upward headroom.
  The input stays ≤ 0.5 V peak and gain stages ≤ ×1.8, and the headroom check enforces it. A 1.75 V reference
  would double the headroom; the brief offers it as an alternative.

**Channels** (each: filter → adjustable gain stage with a 10 k pot, its gain leg returned to VREF, not GND →
test point)

| Channel | Filter | Values (≈) | Gain stage |
| --- | --- | --- | --- |
| A | Sallen-Key low-pass, fc 200 Hz, Q 0.707 | R1 = R2 = 10 k, C1 ≈ 120 n, C2 ≈ 56 n → fc ≈ 195 Hz | ×1 to ×1.8 |
| B | MFB band-pass, f0 1 kHz, Q 2 (passband ≈ 780 Hz to 1.28 kHz) | solver values | ×1 to ×1.8 |
| C | Sallen-Key high-pass, fc 3 kHz, Q 0.707 | C1 = C2 = 10 n, R1 ≈ 3.6 k, R2 ≈ 7.5 k → fc ≈ 3.1 kHz | ×1 to ×1.8 |

**Expected outputs** (0.5 V peak input, gain stages at ×1, ideal filter maths; the simulation will differ a
little through E-series values and the LM358 model):

| Input | Channel A (low-pass) | Channel B (band-pass) | Channel C (high-pass) |
| --- | --- | --- | --- |
| 100 Hz | 0.49 V (−0.3 dB) | 25 mV (−26 dB) | 0.6 mV (−59 dB) |
| 1 kHz | 20 mV (−28 dB) | 0.50 V (0 dB, inverted) | 55 mV (−19 dB) |
| 5 kHz | 0.8 mV (−56 dB) | 52 mV (−20 dB) | 0.47 V (−0.5 dB) |

Each output sits on the 2.5 V bias. The scope shows the swing around it, which is the "distinguish the DC bias
from the AC signal" requirement, already built in Phase 3's net labels.

**Size**: 8 op-amps = 4 × LM358, 1 × 7805, about 14 resistors + 3 pots, about 16 capacitors, 8 test points:
about 45 parts. That fits two full-size (830-point) breadboards. Unit packing (two halves per DIP-8) is a BOM
and placement step that reads the package data (D14). The circuit itself keeps one part per functional op-amp.

## 9. Build order

Each part ends at a gate. Commit conventions are unchanged: one commit per part, the user approves each.

| Part | Build | Gate | Size (est.) |
| --- | --- | --- | --- |
| **P5.0 Spikes and decisions** | Hierarchical compile prototype (3 composites, nested). Behavioral laws → B/G sources with smooth clamps on ngspice native + WASM. XSPICE `--enable-xspice` in the WASM build (size, parity) | Decisions D1–D15 approved (done 2026-10-11); spike numbers in the LLD (done 2026-10-11, LLD §17 as built) | 1 week |
| **P5.1 Core: IR v2** (done 2026-10-11; XSPICE engines moved to the digital track, internal-vector probes to P5.4) | `ComponentDef`, project library, `lib.*` / `instance.expand` / `test.set` / `req.set` / `block.set_spec` ops, v1 → v2 migration, hierarchical compile, pin semantics + ERC on summaries, open refdes prefixes, codegen | Parity CI (1,000 random logs incl. new ops); 20 templates byte-identical; both goldens replay; Phase 1–3 e2e green | 3 weeks |
| **P5.2 Core: laws and value rules** (done 2026-10-11; LLD §17 as built) | Expression language with units, behavioral compile, generic E-series search, SPICE import parser, characterisation points/reports, new check kinds (`tran_headroom`, `ac_db_at`, `tran_rms`, `dc_transfer`, `tran_rise`) | 12 reference black boxes characterised identically native/WASM; 3 templates re-expressed as data with identical ops; fuzzed import parser | 3 weeks |
| **P5.3 Library** | `ref` port + slots on every op-amp template; op-amp/comparator/LDO generators; parts to ~150; templates to ~60; packages; BOM | Every part simulated; every template 5 points; **your analyser buildable by hand** | 3–4 weeks (parallel with P5.4) |
| **P5.4 Editor** | Library panel, component editor (selection / laws / SPICE / AI), generated symbols, open/close instances, drill-down probes, test plan UI and comparison table, BOM view | Playwright flows for each way in; a dry run in which a non-EE user makes a component unaided | 3 weeks |
| **P5.5 Generation v2** | Brief, architect, retrieval (pgvector), slots, gap synthesis, whole-circuit assembly verification, test plans, deliverables | New eval set: 40 complex prompts (yours included) + 20 gap prompts; requirement coverage ≥ 90%, zero requirements marked met without evidence, synthesised components ≥ 70% characterised | 3 weeks |
| **P5.6 Tutor** | Context for instances and black boxes, contract-based explanations, test plan results in context | Tutor evals ≥ 99% refs on new circuits | 1 week |
| **Phase 4** (existing) | Auth, quotas, caches, spend breaker, observability, CSP, load test | As planned, plus the `components` routes in the security review | as planned |
| **D-track (after beta)** | Digital domain per the P5.0 spike: logic family, mixed-signal bridges, 4-bit ALU → small CPU demo | A 4-bit counter and ALU simulate in the browser under the leaf budget | 3–4 weeks |

**Quick wins now (days, they do not pre-empt any decision):** prompt limit to 16k with a clear message; a
potentiometer and a test point part; `unsupported_request` that names each missing piece and the nearest
substitute; `MAX_BLOCKS` 8 → 16 (raised again with D12).

## 10. Risks

| Risk | Mitigation |
| --- | --- |
| Behavioral models fail to converge (hard clamps, `if`) | Smooth limits emitted by the compiler; `gmin`/source stepping on a convergence failure; characterisation catches it before the component is trusted |
| Browser simulation slows down with hierarchy | Leaf budget and Run button (D12); server runs for big designs; ERC on summaries |
| LLM-written components are plausible but wrong | Contracts are simulated, never self-certified; Draft/Characterised badges; the generator uses only characterised components; evals count requirements marked met without evidence |
| SPICE import as an attack surface | Allow-list parser, AST re-emission, size limits, fuzzing, sandboxed sim runner (unchanged) |
| IR v2 breaks recorded cassettes and goldens | v1 migration in core; goldens replay as a P5.1 gate; any re-record is a deliberate, approved step |
| Scope creep toward "any circuit" | Hierarchy and contracts make breadth additive. Out of scope stays explicit: transistor-level CPUs, RF and electromagnetics, thermal, PCB |

## 11. Decisions (user, 2026-10-11: "do the recommended")

1. Platform foundation (P5.0–P5.2) comes before the Phase 4 beta. Phase 4's plan waits; P5.3–P5.6 may run
   before or alongside it.
2. D1: each project embeds the definitions it uses; the user library is where they are copied from.
3. D3: SPICE model import is in v1, behind the allow-list parser and AST re-emission.
4. D11: the XSPICE spike runs in P5.0; the digital track itself stays after the beta.
5. The two collaborator branches (`feat/verify-generated-circuits`, `feat/test-3d-breakboard-feature`) were
   deleted by the user and are not merged. Whole-circuit assembly verification is built here, in P5.5;
   breadboard placement follows D14 when it comes.
