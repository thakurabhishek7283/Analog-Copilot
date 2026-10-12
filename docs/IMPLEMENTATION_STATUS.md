# Implementation status

Where Analog-Copilot stands against its plan. [LLD.md](LLD.md) is the spec and stays the source of truth; this
file only tracks progress. Update it with each commit that finishes a part or a phase.

Last updated: 2026-10-11 (Phase 5, the component platform, approved and placed before the Phase 4 beta; P5.0, P5.1 (`6312a12`, `beae8bc`, not pushed) and P5.2 done (built and verified, commit awaiting approval), next: P5.3 library. Phase 3's 20 student sessions still run whenever the user schedules them).

## At a glance

| Phase (LLD 16) | Gate | Status |
| --- | --- | --- |
| 0 · circuit-core foundation | Parity CI green: same IR and netlist hash | ✅ Done |
| 1 · Local-first editor, no AI | Students build and simulate an op-amp filter unaided | ✅ Done, gate passed |
| 2 · Generation pipeline | ≥ 80% of blocks commit without template fallback | ✅ Done, gate passed (GPT-6 Luna 100%, `cffef9b`) |
| 3 · Grounded tutor | ≥ 99% valid refs; 20 student test sessions | ✅ Built (5 of 5 parts + dry-run fixes); refs 99.8% (golden); the 20 sessions run alongside Phase 4 |
| 4 · Hardening and public beta | All p95 budgets in LLD 1 met → launch | ⏸ Plan proposed; its foundation (P5.0–P5.2) is built |
| 5 · Component platform (LLD 17) | Per part ([plan](COMPONENT_PLATFORM_PLAN.md) §9) | 🟡 P5.0, P5.1, P5.2 done 2026-10-11 (P5.2 gates passed); next: P5.3 library |

**Overall: about 83% of v1.** Weighted by the LLD's own week estimates (P0 2, P1 3, P2 3, P3 2, P4 2 = 12 weeks):
10 of 12 weeks of planned work are built: the deterministic core, the editor, the generation pipeline and the
tutor. What is left is production hardening (Phase 4) and the student sessions.
Content scope (parts and templates, below) is a separate gap the phase table does not schedule.

```text
Phase 0  ██████████  100%
Phase 1  ██████████  100%
Phase 2  ██████████  100%
Phase 3  ██████████  100%  (built; 20 student sessions pending)
Phase 4  ░░░░░░░░░░    0%  (plan proposed; the P5 foundation it waited for is built)
Phase 5  ████░░░░░░   40%  (P5.0, P5.1, P5.2 done; P5.3 next)
```

Phase 5 was added on 2026-10-11 and is not in the 12-week weighting above (about 17 more weeks by its plan).

## Commit history

| Commit | Date | Phase | What |
| --- | --- | --- | --- |
| `6e0c8a6` | 2025-06-15 | 0 | circuit-core with WASM and Python bindings |
| `80200d2` | 2025-06-28 | 0 | Pinned ngspice 47, native and WASM build scripts |
| `1952dcb` | 2025-07-08 | 0 | Every registry part simulated on native ngspice; model fixes |
| `d6d2ecd` | 2025-07-18 | 1 | ngspice WASM simulation worker with watchdog |
| `fed16ba` | 2025-07-27 | 0 | CI: pinned ngspice, simulation tests gate |
| `39ee5eb` | 2025-08-03 | 1 | Schematic symbols in the registry; `changes()` for UI mirrors |
| `77bd88e` | 2025-08-08 | 1 | ngspice COPYING shipped with the WASM |
| `51d11ec` | 2025-08-16 | 1 | Vite + React editor, ELK layout worker, SVG schematic |
| `12b0c15` | 2025-08-22 | 1 | Edit helpers, pin currents, interactive analysis set |
| `200c327` | 2025-08-29 | 1 | Edit tools, scope panel, canvas overlays |
| `b73d645` | 2025-09-03 | 1 | Playwright e2e on the built bundle, in CI |
| `bf7ab83` | 2025-09-08 | 1 | Block templates, value solvers, spec checks |
| `a09db56` | 2025-09-13 | 1 | Every template verified at 5 points, native and WASM |
| `61733a3` | 2025-09-19 | 1 | Insert verified blocks, spec badges, **Phase 1 gate** |
| `20d42f7` | 2025-09-24 | 2.1 | `trial_block`, `bench_ops`, wire types |
| `b0757bb` | 2025-09-30 | 2.2 | sim_runner worker, sandboxed native ngspice, Redis cache |
| `d4b1a71` | 2025-10-06 | 2.3 | API: projects, op log, jobs over SSE |
| `dabdd6a` | 2025-10-10 | 2.3 | CI fix: timeout test under `RLIMIT_AS` |
| `37529b9` | 2025-10-16 | 2.4 | Orchestrator: plan, compose, verify, repair, fallback |
| `431a6b9` | 2025-10-22 | 2.5 | Web: SSE stream, AnimationDirector, sync, generate UI |
| `cffef9b` | 2026-10-08 | 2.6 | Generation evals, golden run replayed in CI (CI failed: see the threshold fix below) |
| `8b83ad3` | 2026-10-09 | 2.6 | Threshold checks at the outermost edge (golden replay fix) |
| `4e5d916` | 2026-10-09 | 3.1 | Tutor context slices, answer reading, wire types |
| `dc002ab` | 2026-10-09 | 3.2 | `POST /ask` over SSE, `asks` table, feedback |
| `9b0b90f` | 2026-10-09 | 3.3 | Ask panel, streamed answers, try-it experiments |
| `5bfdfa8` | 2026-10-09 | 3.4 | "What changed?": core change summary, `/what-changed`, editor buttons |
| `58d6c67` | 2026-10-09 | 3.5 | Core reads `[b2]` as a block reference |
| `7ac7eb8` | 2026-10-09 | 3.5 | Tutor evals: refs, grounding, experiments, rubric; golden replayed in CI |
| `b7b18c8` | 2026-10-09 | 3.5 | `asks` level/effort/context/timings; student-session kit |
| `8f0f9b3` | 2026-10-09 | 3 | Session export names each invalid reference |
| `da5dad6` | 2026-10-09 | 3 | Resizable, collapsible panels; Focus tutor |
| `326ab0a` | 2026-10-09 | 3 | Try-it says whether its prediction held; checks it moved show as retuned |
| `b755422` | 2026-10-09 | 3 | Dropped connection mid-answer is a retryable network error |
| `a67e461` | 2026-10-09 | 3 | Conversation memory, plain-word beginner answers, follow-up evals, new tutor golden |
| `2dab174` | 2026-10-09 | 3 | LaTeX as text, compact block chips, signal swings, part nets on the scope, taller insert form, readable labels |
| `50012d5` | 2026-10-09 | 3 | Facilitator answer key for the student sessions |
| `6fd5395` | 2026-10-09 | 3 | Notes badge, taller inspector, short thinking labels, counting Thinking, example questions |
| `d2f4e31` | 2026-10-09 | 3 | Grounding checker reads arithmetic written in words |
| `de19d2c` | 2026-10-10 | 3 | Check list names unconnected pins in words; steady nets' tooltips without swing |
| `a350765` | 2026-10-10 | 3 | Checker reads a result written before its arithmetic |
| `b5dd37b` | 2026-10-10 | 3 | Selected net brings its blocks; experiment when naming new values; 6000 tokens; tutor golden re-recorded |

Working rule for Phase 2: one commit per part; the user approves each commit and its back-date.

## Phase 0 · circuit-core foundation ✅

- [x] IR types, `apply()` with inverses, unit parser, IR → SPICE compiler (`crates/circuit-core`)
- [x] WASM build (`@tutor/core`) and PyO3 build (`circuit_core`)
- [x] JSON Schema export and codegen to TS and Pydantic (`tools/codegen`, CI fails on a diff)
- [x] First 15 registry parts, each simulated on native ngspice
- [x] Pinned ngspice 47, native and WASM builds
- [x] **Gate:** cross-runtime parity (native vs WASM vs Python, 1,000 random op logs) green in CI

## Phase 1 · Local-first editor, no AI ✅

- [x] Schematic view: SVG symbols, canvas overlays (voltage colouring, current dots), pan/zoom
- [x] ELK layout worker (block-level cache, deterministic block placement)
- [x] ngspice WASM simulation worker with 2 s watchdog
- [x] Edit tools (place, wire, rails, drag/pin, rotate, delete), undo/redo, inspector
- [x] Scope (uPlot): transient and AC/Bode tabs, probes
- [x] 20 verified block templates with Rust value solvers and `.meas` spec checks, verified at 5 points each
- [x] Playwright e2e on the production bundle in CI (11 flows)
- [x] **Gate:** a student builds and simulates an op-amp filter unaided, both from a block and from parts (LLD 16, as built)

## Phase 2 · Generation pipeline ✅

- [x] **Part 1 · core** (`20d42f7`): `trial_block` (a draft block tried in its template's verification bench), `bench_ops`, wire types for REST and job events
- [x] **Part 2 · sim_runner** (`b0757bb`): arq worker, native ngspice under rlimits, Redis result cache, Docker image with selftest
- [x] **Part 3 · API** (`d4b1a71`, CI fix `dabdd6a`): FastAPI, anonymous sessions, projects and op log on Postgres, jobs streamed over SSE from Redis Streams, reaper, `JobContext`, compose stack
- [x] **Part 4 · orchestrator** (`37529b9`): planner, composer, verifier, repair (3 attempts), template fallback, narration; LLM gateway with an OpenAI-compatible adapter, fake provider, cassettes; prompts (≈ 38k input tokens per 4-block circuit by estimate, inside the 40k budget)
- [x] **Part 5 · web** (`431a6b9`): SSE client, AnimationDirector on the Web Animations API, sync with an IndexedDB queue, generate UI, Playwright `local` + `api` projects (17 flows)
- [ ] **Part 6 · evals and gate** (in progress; see [evals/README.md](../evals/README.md))
  - [x] `evals/prompts.yaml`: 100 prompts (every template at 2+ levels, 26 chains, vague, injection, 5 out of scope), each with the templates its plan should use
  - [x] `evals/run_evals.py`: each prompt a real job on the real API (testcontainers, sim worker, SSE), 4 at a time; reads block outcomes from `jobs.plan.blocks[].outcome`; logs every model call (provider token counts, cached tokens, latency); `--record` and `--replay` cassettes; real providers only with `--live`
  - [x] `evals/metrics.py`: the LLD 15 metrics, plan match, out-of-scope refusals, by role and level; `report.json` and `report.md`
  - [x] Gate: ≥ 80% without fallback; vs baseline, commit rate −2 points or tokens +15%; any cassette miss
  - [x] CI: harness tests on scripted replies; the gate step replays `evals/golden` once it exists
  - [x] Tested: 15 harness tests (a recorded run replays to identical outcomes); all 100 prompts run on the e2e fake script in ~16 s
  - [x] Exchange provider (`evals/exchange.py`): measures replies from any chat (Gemini or DeepSeek web, Claude, a person). The run writes paste-ready prompts to `pending/`, you save the replies, and you rerun until nothing waits. Replies are checked like an API's and frozen once used; tokens are estimated and latency is not measured
  - [x] Authored exchange run (`evals/authored/`): Claude answered all 100 prompts as the model (6 rounds, 337 replies). 95/95 committed, every plan valid first time, 5/5 out-of-scope refused, 173/173 checks passed. Every block kept its template, so drafting, repair and fallback were not exercised
  - [x] Decided (user, 2026-10-08): `use_template` keeps counting toward the gate; the report shows drafting next to it, not gated (the share of blocks drafted, and the share of tried drafts that committed)
  - [x] Live runs approved (2026-10-08): each provider on its own, no fallback, recorded to `evals/results/<name>-live/` (gitignored: report.json, report.md, cassette.json)
  - [x] Adapter: Gemini's 429 body read (retry delay, quota id; a per-day quota is `quota_exhausted`, not retried); `<P>_REASONING=1` for reasoning models (`max_completion_tokens`, no temperature)
  - [x] Evals pace rate limits (`--max-wait`, default 900 s per call for live runs; `--job-timeout` 3600 s); job latency is not reported when waits happened
  - [x] **GPT-6 Luna (Azure) live run: gate passed.** 100% of 139 blocks without fallback, 99.3% first attempt, 1.01 attempts per block, 0.6% spec error, 15.7k in / 515 out tokens per circuit, 95/95 done, 5/5 out of scope refused, about 85% of input tokens cached. Drafted only 4 blocks (it keeps templates); 1 spec miss repaired. 60 Azure rate-limit waits
  - [x] **DeepSeek-V4-Pro (Azure) live run: gate passed.** 100% of 140 blocks without fallback, 100% first attempt, 48% of blocks drafted by the model itself (all passed first time), 0.6% spec error, 15.3k in / 514 out tokens per circuit, 95/95 done; model latency 3.1 s median per compose call. 678 Azure rate-limit waits (110 min) made the run take 54 min. Found: 2 of 5 out-of-scope requests (oos-fm, oos-class-d) ended `plan_invalid` instead of `unsupported_request` (the model planned blocks as well as listing what is uncovered); fixed below
  - [x] Planner fix (user, 2026-10-08): uncovered items kept to the last plan make the job `unsupported_request`, whatever else that plan got wrong (LLD 6). Requests unchanged: DeepSeek's cassette replays with 5/5 refused
  - [ ] Gemini: blocked by the free tier (5 requests per minute and about 20 per day for `gemini-3.5-flash`). Needs billing on the AI Studio project, or measure it through the exchange workflow
  - [x] Missing `targets` (user, 2026-10-08): a plan block without `targets` parses as all defaults; the schema sent is unchanged (still required), so the test cassettes stay valid. Luna's run must be redone (2 prompts no longer replay)
  - [x] Production default (user, 2026-10-08): GPT-6 Luna; its fresh run after the fixes goes into `evals/golden/`
  - [x] Cassettes record a call the job cancelled (`cancelled`; its replay waits to be cancelled again): a narration still waiting out a 429 when its job ended had left the cassette, and the replay missed it (DeepSeek 26, the first post-fix Luna run 4)
  - [x] **GPT-6 Luna after the fixes (`luna-live-3`), accepted into `evals/golden/` (user, 2026-10-09): gate passed live and on replay.** 100% of 138 blocks without fallback, 99.3% first attempt, 1.04 plan calls per prompt (was 1.17), 14.9k in / 495 out tokens per circuit, 95/95 done, 5/5 out of scope refused, 2.2% drafted (3 of 3 passed). 55 rate-limit waits; 4 cancelled narrations recorded. Superseded by the run below
  - [x] Renamed Circuit Forge to Analog-Copilot (user, 2026-10-09), the model prompts included: test cassettes re-recorded (replies unchanged), authored exchange redone from its frozen replies (identical outcomes)
  - [x] **GPT-6 Luna on the renamed prompts (`luna-live-4`), accepted into `evals/golden/`: gate passed live and on replay.** 100% of 137 blocks without fallback, 99.3% first attempt, 1.05 plan calls per prompt, 14.9k in / 482 out tokens per circuit, 95/95 done, 5/5 out of scope refused, 6.6% drafted (9 of 9 passed). 63 rate-limit waits; 7 cancelled narrations recorded. The CI gate step replays it
  - [x] Licence: PolyForm Noncommercial kept; the MIT `LICENSE` from the remote's README commit removed (user, 2026-10-09)
  - [ ] Optionally re-record the orchestrator test cassettes from live replies
  - [x] Commit part 6: `cffef9b`, pushed. CI failed only at the golden replay: on Linux ngspice the schmitt-2v5 bench read Vth+ 3.1 V as 2.21 V (a spurious switch inside the hysteresis; reproduced in the local sim-runner image), so the job asked for a repair the cassette does not hold
  - [x] Threshold checks fixed (`8b83ad3`, pushed): `tran_threshold` takes the outermost of up to 4 edges in the second half of the run (highest input at a fall, lowest at a rise); `SpecCheckDef.edge` added (codegen). Regression test in tools/sim at 1.9/2/2.1 µs steps; checked on Linux ngspice in the sim-runner image; golden replay passes unchanged (cassette still valid)
  - [ ] **Gate:** ≥ 80% of blocks commit without template fallback

Dropped from Phase 2 by the user (2026-10-03): the spend guard. The daily spend breaker stays in Phase 4.

Also done on 2026-10-08, outside the phase plan:
- Licence: PolyForm Noncommercial 1.0.0 (`LICENSE.md`, `Required Notice: Copyright 2025-2026 Abhishek`; the user may put a full legal name), declared in every manifest; `THIRD_PARTY_NOTICES.md` generated by `tools/licenses/third_party.py`
- `tools/e2e/stack.py --port 8000 --llm-from .env` runs the API locally with the real provider in `.env` (no image builds). `docker compose up --build` failed on this machine: Debian's apt mirror served files that failed their hash checks (a network/proxy issue)
- `compose.yaml` passes `OPENAI_BASE_URL` and `OPENAI_REASONING`; `.env` now selects GPT-6 Luna (`LLM_PROVIDER=openai`, Azure v1 endpoint, `OPENAI_REASONING=1`)

## Phase 3 · Grounded tutor ✅ (built; sessions pending)

Plan (proposed 2026-10-09, one commit per part): 1 core (`circuit_core::tutor`: context builder, refs, try-block validation, wire types), 2 API (`asks` table, `POST /ask`, feedback, safety notes), 3 web (AskPanel, click-to-ask, ref chips, Socratic toggle, try-it cards), 4 what-changed, 5 tutor evals and the student-session kit.

Decided (user, 2026-10-09):
- `/ask` streams its answer directly over SSE (no Redis Stream, no resume; a dropped stream asks again); the answer is saved to `asks` when it ends. A deviation from LLD 5
- Model tier per kind (ask, what-changed) is configurable; the tutor evals measure small and large, then the user picks
- The client sends all simulation values (every net's OP voltage, pin currents, check results, values at the scope's point); the server's core picks the slice from its snapshot at `rev`
- Mains safety notes are deterministic: the core flags hazard parts, the server appends a fixed note; tested with a test registry part (none is flagged yet)
- Defaults: applied try-it ops are `user` ops; tutor answer cache, quotas and `usage_daily` stay in Phase 4

- [x] **Part 1 · core** (`4e5d916`): `circuit_core::tutor` with `context` (selection rules, 1-hop neighbours over signal nets, named parts, 15 parts / 2k tokens, ERC, hazards) and `read_answer` (refs with UTF-16 spans, `try` blocks validated on a scratch apply); wire types `AskRequest`, `AskEvent`, `AskFeedback`; both bindings; 10 Rust tests (one insta snapshot) plus binding tests; LLD 9 as built
- [x] **Part 2 · API** (`dc002ab`): migration `0002` (`asks`: nullable `selection`, plus `mode`, `model`, tokens); `load_session(at=rev)` (snapshot or a fold from the start); `tutor_api/tutor` (core context, `ask` request on the small tier, `LLM_TIER_ASK`, fixed mains safety note); `POST /v1/projects/{id}/ask` (checks in a dependency before the stream: 503 `tutor_unavailable`, 409 `rev_not_synced`, 422 core codes; `answer.delta`/`answer.done`/`error`, `llm_unavailable`, `tutor_timeout` 60 s; saved when complete; disconnect cancels the call) and `POST /v1/asks/{id}/feedback` (204); prompts `tutor_system.jinja`, `ask.jinja`; compose passes `LLM_TIER_ASK`; 6 API tests + 3 service tests; LLD 5, 9, 11 as built. Verified: API suite 85 passed, evals tests 20 passed, golden replay gate passed. Live check run (user, 2026-10-09; scratchpad script, 4 questions): found and fixed `pass_` sent to the core instead of `pass` (Pydantic dump without `by_alias`; the API test now sends check results); 15/15 refs valid; the 500 Hz `try` predicted 498 Hz, re-simulated 499 Hz; Socratic and injection fine. Then (user, 2026-10-09): latency is not a concern and the learner picks the reasoning effort (`AskRequest.effort` low|high, `reasoning_effort` to reasoning models, cassette keys unchanged when unset); Ask calls get 120 s each and 180 s per answer (`LlmRequest.timeout_s`); `ASK_MAX_TOKENS` 4000. Slice: "whatever is less error-prone" → a selected or named part brings the rest of its block before outside neighbours (core, snapshot updated). Second live run: low 2.3 s, high 14 s and a correct derived fc/Q; all refs valid. Verified after: Rust workspace, parity 1000, bindings, API 88, evals 20, golden replay gate. Still open: prefix under the 1,024-token cache minimum (minor). The fake script's Ask rules come with part 3
- [x] **Part 3 · web** (`9b0b90f`): `tutor/` (askStream over fetch SSE, simValues from the latest result: finite only, tran over the second half, AC at the slowest sine source only inside the circuit's own sweep; tutorStore; asker: flush, wait for the sim, one question at a time, stop, feedback, Try it as one undo step then measured, Undo it), `AskPanel` (About <selection> chip, Explain/Guide me, Normal/Quick/Deep thinking, level; ref chips select; invalid refs plain text; experiment card with checks before → after; Yes/No feedback), `ApiClient.feedback`, `Editor.tutor` + `readAnswer`; core doc: tran range after start-up (codegen); fake script Ask rules (e2e-ask, -fail, -slow, default note); `stack.py` passes the tutor; Playwright `api` project runs `ask.spec.ts` (3 tests). Fixed on the way: the side column squashed the Ask form under the lesson (min-height). Verified: tsc, vitest 100, Playwright 20/20 (both projects), API + evals 108, Rust fmt/clippy/test, codegen. Screenshots checked light and dark. Re-verified before the commit: also parity 1000 and the golden replay gate
- [x] **Part 4 · what changed** (`5bfdfa8`). Design approved (user, 2026-10-09): on click only; its own endpoint `POST /v1/projects/{id}/what-changed` with `ChangeRequest {from_rev, rev, before, after, level, mode, effort?}`. Core `tutor::changes` (both bindings, `tutorChanges`/`tutor_changes`): EDIT (params, swaps, pins, whole blocks as one line, renamed nets, analyses, "nothing electrical"), BLOCK with `teach`, SIM, CHECKS before → after, VOLTAGES that moved >1% and 1 mV (or 1°), largest first, at most 12, CURRENTS of edited parts, ERC new/resolved, SAFETY; 2k-token budget. API: both endpoints share one stream (`answered`), `Tutor.changes`/`change_request`, `LLM_TIER_WHAT_CHANGED` (compose), `what_changed.jinja`; the tutor system prompt describes both inputs (prefix ≈1,155 est. tokens, past the 1,024 cache minimum; to confirm live); migration `0003` (`asks.kind`, `asks.from_rev`). Web: `CircuitState.changes` (labels with revs; Undo/Redo named), `tutor/simHistory.ts` (last 10 settled simulations, `changeAt`), `asker.whatChanged`, panel button "What changed? · <change>" (hidden once asked) and one on a measured Try-it card; fix: a result arriving after a newer edit stays `pending` (it read `ok` at the new rev for up to 150 ms). Fake script rules (`what_changed`). Tests: Rust 4 (+1 snapshot), bindings 1+1, API 2 + service 1, vitest 4 (sim race, simHistory 2, asker), e2e ask flow extended (card → explained, panel button after Undo). Screenshots light and dark checked
- [x] **Part 5 · tutor evals and the student-session kit** (`58d6c67`, `7ac7eb8`, `b7b18c8`; not pushed). Design approved (user, 2026-10-09, "your recommendations, simplest"): 120 cases (90 ask, 30 what-changed; 50 rubric: 35 + 15); judge DeepSeek-V4-Pro; tiers both Luna, so the matrix is effort low vs high; `asks` gets level, effort, context, timings (migration `0004`, no `/tried` endpoint: a Try-it applied is read from the op log); grounding gated vs baseline only; commits: core fix, evals, sessions
  - [x] `evals/tutor/`: 26 circuits (`circuits.yaml`), 120 cases (`cases.yaml`), `fixtures.py` + `sim_values.mts` freeze each circuit's simulation values made by the editor's own store/scheduler/`simValues` on ngspice.wasm in Node (`fixtures.json`)
  - [x] `run_tutor_evals.py` (real API on testcontainers; `--tier`, `--effort`, `--rubric`, `--judge`, `--judge-live`; replay takes tier/effort/rubric from `--baseline`), `tutor_metrics.py` (refs, grounding with shown arithmetic evaluated, one-step class, experiments re-simulated on native ngspice, gate, report, `compare`, `agreement`), `tutor_judge.py` (6 criteria; only deterministic facts in its prompt); CI step "Tutor evals gate" (replays `evals/tutor_golden` once it exists); README section; LLD 15 as built
  - [x] Tests: metrics 9, fixtures 123 (every case's context builds inside the budget, fixtures match the YAML), harness 2 (scripted run checked and graded; record → replay identical), session export 1
  - [x] Live runs (user approved low + high, 2026-10-09; GPT-6 Luna on Azure, DeepSeek judge): `evals/results/tutor-luna-low`, `tutor-luna-high`. Before the checker fixes: refs 98.4% / 99.5%, rubric pass 92% / 98%, experiments when expected 69% / 94%, out tokens 187 / 744, answer p50 2.9 s / 6.1 s. Prompt cache confirmed (62% of Ask input, 78% of what-changed input). None of the 8 invalid refs (low) was invented: 5 `[b2]` block ids without the prefix, 3 parts the explained edit removed
  - [x] Fixes from the live runs: core reads `[b2]` as a block (`parse_ref`); removed parts named in a change summary count as valid in the evals; checker: femto, LaTeX, `∥`, arithmetic after the result, tolerance band edges, a bare 39 for 39kΩ, the experiment's own values; one-step class reported apart; prediction compares every number with every moved check (source checks left out); judge prompt without checker or ngspice facts
  - [x] Rescored both runs by replay; regraded with the new judge prompt (`--judge-live`, user approved, ~100 DeepSeek calls): low vs high: refs 100% / 100%, grounded answers 89% / 89%, experiment when expected 69% / 94%, predictions held 100% / 94%, rubric 90% / 96%. High accepted as `evals/tutor_golden` (user); replays through the gate
  - [x] Verified: cargo fmt/clippy/test, parity 1000, bindings (node 9, py 11), codegen, evals + API 247, vitest 104, tsc, build, Playwright 20/20, generation golden replay gate, tutor golden replay gate
  - [x] Committed (user approved, not pushed): `58d6c67` fix(core) [b2] refs; `7ac7eb8` feat(evals) tutor evals + golden; `b7b18c8` feat(api) ask details + session kit
  - [x] 5b: migration `0004` (`asks.level`, `effort`, `context`, `first_token_ms`, `ms`), saved by both endpoints; `stack.py --database-url --redis-url` (compose's databases outlive the API); `evals/sessions/` (README guide and criteria, tasks, observation sheet, survey, `export.py`)
  - Findings for later: Luna at low effort sometimes writes LaTeX (`\(9\times4.9\)`) in a plain-text answer; sub-femtoamp currents print as `0.000148fA`; grounding an inverting amp's input reads "gain frequency outside the AC sweep"; a selected block brings no neighbours, so the diff amp's V2 is not in its slice (its net values are)
- [ ] **Student sessions** (setup 2026-10-09): compose Postgres/Redis up (`tutor` empty, `tutor_dryrun` for rehearsals); real API on :8000 (`stack.py --llm-from .env` on compose's databases), session app :4173; free rehearsal: fake stack :8100 on `tutor_dryrun` + preview :4174 (`API_ORIGIN=http://127.0.0.1:8100`). Fake rehearsal through export.py passed (sessions split, refs, Try-it applied, Yes/No). Export fixed (`8f0f9b3`): names invalid refs (folds the op log to the answer's rev), 0 ms timings kept. Paid dry run: not yet (user runs it)
  - [x] Panel layout (user, 2026-10-09: "too cluttered, I should be able to resize the tutor or change the layout"; chose resizable + collapsible): `Workspace.tsx`, `panels.ts`, `Splitter.tsx`; drag handles (columns, scope, side panes; keyboard; double-click resets), collapsible panes and palette, Focus tutor (≥45% width), per-browser localStorage, Reset layout; fixed the Ask form overlapping the lesson at 1366×768; schematic refits on resize. Verified: tsc, vitest 111, build, Playwright 22/22 (+2 layout). LLD 10 as built. Committed `da5dad6` (not pushed)
  - [x] Paid dry run (user asked Claude to run it, 2026-10-09 19:56): tasks 1-5 + Deep question as a first-year, Playwright driving :4173, 9 answers (Luna, normal effort except one Deep). 0 wrong, 2 weak, 100% refs (43), both Try-its applied and measured (498 predicted / 499 Hz; C1 reconnect restored 996 Hz / Q 0.706); first token 4-7.5 s
  - UX findings from the dry run, by severity (none fixed yet):
    - HIGH no conversation memory: each question goes alone (ask.jinja), so Guide me never confirms a right answer (asked the same question again after the student said 0.705 ≈ 0.707, -3 dB) and follow-ups ("explain that more simply") are guessed. Fix: send the last few turns; wire type + prompt change, so a new live tutor-eval run (paid)
    - HIGH a deliberate retune reads as failure: after the 500 Hz Try-it the badge shows fc 499Hz ✗ in red (target still 1 kHz); the card shows 499Hz red and never says the prediction held
    - MED beginner answers too technical: formula first, node lists, phases, "operating-point current", jargon (ERC, "C1.2 is marked -")
    - MED raw LaTeX in answers (`\(1/\sqrt{C2}\)`), also at Deep thinking
    - MED block chips repeat the title after the model's own words ("Sallen-Key filter Sallen-Key low-pass (2nd order)", 3x in one answer)
    - MED insert form crammed in the 200 px inspector pane (Insert block button below the fold)
    - MED schematic tiny at 1366x768: 6-7 px labels, a part click missed twice (between C2's plates deselects)
    - MED wires and pins read "0 V" (DC operating point) while a 1 V sine drives the circuit
    - MED selecting a part leaves the scope empty ("Select a net or a block")
    - LOW "1 check" badge is an INFO (U1 unit B unused); inspector pane short (pins cut off); Ask button wraps at 400 px; pin disconnect button named "×"; static "Thinking…" for 4-8 s; empty Ask pane gives no example questions; checker flags arithmetic written in words (884 Hz)
    - KIT the facilitator (user) is not an EE: add an answer key to tasks.md; Claude reviews answers.csv as the reviewer
  - [x] Second paid dry run (user approved, 2026-10-10 00:25 local; Claude as a first-year beginner, Playwright on the real API, project 9b50f549): 8 answers, 0 wrong, 0 weak, refs 100% (30), both Try-its applied and measured (500 Hz: ✓ Prediction held 498/499 Hz, badge "fc 499Hz (retuned)"; the C1 fault fixed back to 996 Hz / Q 0.706), answers 7.4 s median, memory works (follow-up built on the 1.34 kHz; Guide me confirmed "Yes, about 0.7: 705 mV ÷ 1 V = 0.705"), What changed? on C2 → 10 nF right (438 Hz −3 dB, Q 0.49), the fault named in plain words ("C1 has one end left unconnected"), Deep in 10 s. UI fixes all seen working (1 note badge, ± swings, part nets on the scope, example questions, Thinking… N s, Insert reachable, compact ⌖ chip). Findings, all LOW: partly right / wrong Socratic openers (known); the model sometimes stacks 3 chips in a row ("the cutoff B1_IN B1_OUT <block>"); rail tooltip "swings 12 V to 12 V" (fixed); check list "not connected: C1.2" (fixed: "C1 pin 2" in the UI); checker missed "36 kΩ = 18 kΩ × 2" (fixed). Fixes committed `de19d2c`, `a350765` (not pushed). Export the real sessions with --since after 2026-10-10T00:40 local
    - NEW (found fixing 2): a −3 dB check reads wrong when the cutoff falls below the AC sweep's start: the RC low-pass retuned to 159 Hz measured 324 Hz (its sweep starts near 200 Hz and the check measures from the first point). Matters for big retunes only (500 Hz on the 1 kHz Sallen-Key measured 499 Hz)
  - Fixes 1-3, plan approved (user, 2026-10-09): memory from the server by ask id (not client text); retune shown neutral + prediction held (no new-target op)
    - [x] Fix 2 (`326ab0a`, pushed): the card compares prediction and measurement by the evals' rule, ported to TS (`tutor/predict.ts`) with shared cases (`evals/tests/prediction_cases.json`, run by vitest and pytest); a number after a bound ("below 2 kHz") is not compared (both readings; golden unchanged: 94%, 23 with a number). Green "✓ Prediction held", neutral "Prediction missed"; checks the experiment moved show "(retuned)" in a neutral colour on the badge and inspector while they still read the measured value. Fake experiment R1 → 16k ("about 1 kHz"). Verified: tsc, vitest 129, build, Playwright 22/22, API+evals 247, both golden replays
    - [x] Fix 1 built: `AskRequest.history` (ids, ≤3; codegen), server loads them from `asks` (own, same project; else 422), `Turn` (try block → one line, 1,500 chars, "before later edits"), `ask.jinja` "The conversation so far", system prompt conversation rules, Socratic line confirms first; migration `0005` `asks.history`; web sends the topic's last 3 saved answers, "New topic" button + divider; export `follows` column, grounding counts earlier turns
    - [x] Fix 3 built: system prompt level section (beginner: idea first, 1-2 numbers, no lists, no ERC/operating point/DC bias/quiescent/pin names/"-"); evals count beginner jargon (reported, not gated); judge's level and mode rules updated
    - [x] Evals: 10 follow-up cases (130 total, 60 rubric), harness asks earlier turns live (`turns`, `after`), judge sees the conversation; scripted harness test covers both shapes, record → replay identical
    - Free checks passed: cargo fmt/clippy/test, parity 1000, bindings (node 9, py 11), codegen stable, API+evals 258 + tutor unit test, vitest 132, tsc, build, Playwright 23/23, generation golden gate. Tutor golden replay: 130/130 cassette misses, as expected (prompt changed)
    - [x] Paid run 1 (`evals/results/tutor-luna-high-2`, user approved): 129/130 (one `ssl.SSLError` mid-stream reached the router as `internal`: adapter now maps OSError to a retryable `network` error, test added); refs 100%, rubric 95%, beginner jargon 0% (was 33%), but answers fully grounded 78% (beginners dropped the arithmetic; one wrong Q 1.88); follow-ups 8 good, 2 weak. User chose: tweak prompt, rerun
    - [x] Prompt tweaks: every worked-out number keeps its arithmetic (beginners: one line after the idea); partly right opens with the right part, wrong with a plain "no" (neutral examples, not from the cases)
    - [x] Paid run 2 (`tutor-luna-high-3`): 130/130, refs 100%, quantities grounded 97%, answers 83% (93% incl. one step), predictions 100%, rubric 98%, jargon 2%. Accepted as `evals/tutor_golden` (user, 2026-10-09); replay gate passes
    - Open after it (next measured fix, repeat the follow-up and spec cases several times rather than a full rerun): (1) experiment when expected 65% (values in words, no try block; one answer cut at the 4,000-token limit: raise ASK_MAX_TOKENS to 6000 with the next re-record, it changes the cassette key); (2) partly right / wrong openers still "Not quite" alone; (3) a selected output net's slice lacks the block's input level, so a right reply about 0.707 of the input was called partly right (core context change)
    - [x] Committed (user approved, pushed by the user): `b755422` fix(api) dropped connection; `a67e461` feat(api) conversation + beginner answers + new tutor golden. codegen --check passes
  - Findings 4-9 and the answer key (no model cost), built: (4) LaTeX read as text in the client (`tutor/latex.ts`); (5) block chips compact after the model's own words or a first chip; (6) inspector ≥460 px while inserting + sticky Insert/Cancel; (7) labels/badges ≥9 px on screen (`--label-scale`, ≤1.3×), value as a second line, 4-unit part hit pad; (8) nets/pins/ports show the transient swing (±705 mV; 6.63 V ±1 V), operating point in the tooltip; (9) a selected part's signal nets on the scope; KIT `evals/sessions/answer_key.md` (separate from tasks.md: students read the task card), linked from the README. Verified: tsc, vitest 139, build, Playwright 23/23 (ask: LaTeX + compact chip; blocks: part → scope, ±; editing: part follow), test_llm + test_run_evals 28; screenshots at 1366x768 checked. Committed (user approved, pushed by the user): `2dab174` feat(web), `50012d5` docs(evals) answer key
  - LOW findings 10-16, built: (10) notes-only ERC badge neutral "1 note"; (11) inspector default 280 px; (12) thinking labels Normal/Quick/Deep, Ask fits at 400 px; (13) disconnect button aria-label; (14) "Thinking… N s"; (15) example questions in an empty panel (fill, not send); (16) checker reads arithmetic in words ("gives about", "comes to") and "N times (a ÷ b)" (dry-run 884 Hz now grounded; tutor golden replay unchanged, gate passes). Verified: tsc, vitest 139, Playwright 23/23, metrics/export/harness tests 13, screenshots at 1366x768. Committed (user approved, not pushed): `6fd5395` feat(web), `d2f4e31` fix(evals)
  - The three open tutor items (user chose: fix first, 2026-10-09): (3) core: a selected net brings its parts' blocks (the Guide-me slice now has the 1 V input); (1) prompt: naming new values for a target ends with the `try` that sets them; (2) prompt: partly right opens with the right part, wrong with "No"/"Not quite", a value the student stated counts as given; ASK_MAX_TOKENS 6000. Targeted paid check (user approved, 2 × 27 answers): try when expected 16/17 both runs (was 65%), dry-run Guide-me confirmed both runs, openers unchanged (accepted as minor). Full re-record (user approved; `tutor-luna-high-4`): 130/130, refs 99.8%, grounded answers 86%, try when expected 94%, predictions 100%, rubric 97%, jargon 0%. Accepted as `evals/tutor_golden` (user, 2026-10-10); replay gate passes. Verified: parity 1000, bindings, Rust, API+evals 260, web 139 + Playwright 23/23, both golden gates. Committed `b5dd37b` (not pushed)
- [x] Context builder (LLD 9): selection, live simulation values, block/template context (parts 1-3)
- [x] `POST /ask` with refs, streamed; `AskPanel` and click-to-ask in the editor (parts 1-3)
- [x] Try-it ops (the tutor proposes edits the student can apply) (parts 1-3)
- [x] What-changed diffs (part 4)
- [x] `asks` table and migration (part 2)
- [~] Tutor evals: reference validity, automatic numeric grounding, a weekly LLM-rubric sample (part 5; weekly scheduling waits for Phase 4)
- [x] Safety notes for `hazard: mains` parts (part 2: the tutor's fixed note; generation refusing mains builds for beginners is separate, LLD 14)
- [ ] **Gate:** ≥ 99% valid refs (✅ 99.8%, golden); 20 student test sessions (run alongside Phase 4, user schedules them; export `--since` after 2026-10-10T00:40 local, review against `evals/sessions/answer_key.md`)

Decided (user, 2026-10-10): Phase 3 counts as built; the sessions do not block Phase 4. Minor items left open (fix only when asked): Guide me's bare "Not quite" opener; three reference chips in a row; about 1 in 70 high-effort calls thinks past 120 s and is retried; a −3 dB check below the AC sweep's start; the grounding checker drops V/µs, mV, siemens.

## Phase 4 · Hardening and public beta 🟡

- [ ] Auth: OAuth or email magic link, 15-minute JWT and rotating refresh cookie (replaces anonymous sessions)
- [ ] Quotas: Redis token buckets per tier; `/me/usage`; `usage_daily` table
- [ ] Caches: plan cache, whole-circuit replay, tutor answer cache
- [ ] Daily spend breaker, switching to template-only mode (the mode itself already exists)
- [ ] Observability: OpenTelemetry traces per job, Langfuse, metrics and alerts (LLD 15)
- [ ] Registry manifest redirect and CDN deployment
- [ ] Strict CSP in the browser
- [ ] Load test: 500 open SSE streams per API pod; sim queue under burst
- [ ] Sandbox and security review
- [ ] **Gate:** every p95 budget in LLD 1 met → launch

## Phase 5 · Component platform 🟡 (approved; runs before the Phase 4 beta)

Plan: [COMPONENT_PLATFORM_PLAN.md](COMPONENT_PLATFORM_PLAN.md); decisions in LLD 17. Why: a long specification
(the user's three-band analyser on a 5 V LM358 supply) fails on the 2,000-character prompt limit, 8 blocks,
TL072/±12 V-only templates, no reference port, no potentiometer or test point, no multi-frequency test, and
any block the library lacks is refused because new templates need a Rust solver.

Decided (user, 2026-10-11, "do the recommended"): foundation (P5.0–P5.2) before the Phase 4 beta; projects
embed the definitions they use; SPICE model import in v1 behind the allow-list parser; the XSPICE spike in
P5.0, digital simulation after the beta. The collaborator branches `feat/verify-generated-circuits` (`71bbeb1`)
and `feat/test-3d-breakboard-feature` (`441e567`) were deleted by the user, unmerged (so was `origin/dev`;
`origin/HEAD` now points to `main`); whole-circuit verification is rebuilt in P5.5.

- [x] **P5.0 · spikes** (2026-10-11; harness `tools/spikes/p5_0`, uncommitted; results LLD 17 "As built"):
  - Hierarchy: flat, value-specialised `.subckt`s and `params:` identical on all 3 engines (Windows, Linux image, WASM), engines within 2e-6; probes inside instances work by `.meas`. Chosen: value-specialised `.subckt`s
  - Scale: hierarchy free; 0.21 ms/device in WASM → live budget about 1,000 leaf devices (D12 corrected from 2,000), server about 8,000
  - Laws: bound every limited state (without it a Schmitt switched at 4.34 V, not 2.83 V); hard limits, not smooth (5.6× slower, 27 mV bias); transfer function as a state space exact. The bounded law op-amp is identical on all engines to 6 digits where the shipped `lm358.lib` differs by 6% (2.83/2.67/2.66 V): P5.3's generator emits bounded models
  - XSPICE: WASM with static code models works (+86 KB gzip, +3.5% time, 112 analog decks bit-identical, digital deck exact and equal to Windows); 1,920 digital instances × 1,000 clocks 3.9 s. Open: Linux native XSPICE build (moved into P5.1)
- [x] **P5.1 · core, IR v2 and editor** (2026-10-11, `beae8bc`, not pushed; plan and spikes `6312a12`; LLD 17 "As built (P5.1)"): project library (`ComponentDef`, composite model, `dc_group`), refdes prefixes, ops `lib.add/update/remove`, `instance.expand`, `block.set_spec`, `test.set`, `req.set`; schema 2 + `migrate`; nested `.subckt` compile, `leaf_devices`; ERC from pin semantics; `MAX_BLOCKS` 32; auto symbols in the core; store, layout, schematic and inspector draw library parts
  - Verified: cargo fmt/clippy/test (all Rust suites, incl. 12 library tests and auto symbols), parity 1000 with the new ops, bindings (node 9, py 11), codegen, tools/sim 144 (+2: nested = expanded = flat, WASM agrees), API 94, evals 166, sim_runner 15, both golden replays pass unchanged, tsc, vitest 141, build, Playwright 24/24 (+1 API flow: a component through `POST /ops`, drawn and simulated)
  - Moved (decided 2026-10-11): XSPICE in both engines to the start of the digital track; drivers keeping requested internal vectors to P5.4 with the drill-down probes
- [x] **P5.2 · core, laws and value rules** (2026-10-11, built and verified; commit awaiting approval; LLD 17 "As built (P5.2)"). Plan approved (user: "continue and decide for yourself"): one commit; characterisation in the core and the test harness now, the server job and route with the `components` table in P5.4; Sallen-Key as the third template
  - Built: `expr` (law and SPICE dialects, dimensions, evaluator, emitter), `rules` (generic E-series search, slots), templates `rc_lowpass`, `rc_highpass`, `noninverting_amp`, `sallen_key_lp` as `rules:`, params on library components and value-specialised `.subckt`s, `behavioral` (laws, `tf` state space with C = 1/ω0, clamped states bounded, `I(pin)` sense sources, requires: after a transient and by ERC008 on known rails), `spice_import` (allow-list parser, `.param`/`.func`/`PARAMS:` evaluated by the core, POLY and `VALUE=` to B-sources, everything renamed), check kinds `tran_headroom`, `ac_db_at`, `tran_rms`, `dc_transfer`, `tran_rise` and `compare`, `Unit::Decibel`, `characterise` (contracts, points, benches, reports), both bindings, codegen
  - Gates: 12 black boxes (`tools/sim/blackboxes.yaml`, 56 points, 128 checks) characterised on Windows, WASM (worst 4e-5) and the Linux image (no difference); 4 templates as data give the Rust solvers' values in 820 of 820 cases; import parser fuzzed (6,000 proptest cases: random text, token soup, mutated models; allow-listed output, re-emission a fixed point). Also: registry LM358, TL072, LM7805, NE555 imported simulate as their `.include`
  - Found on the way and fixed: the importer checked units on vendor expressions (fuzz corpus, NE555's `tanh`); bench refdes must take plain prefixes (`R1`, not `RL`); PyYAML reads a check named `off` as false; the property test reloaded the registry per case (now 2 s, was 18 s)
  - Verified: cargo fmt/clippy/test (Rust suites incl. components 5, rules 2, spice_import 4), parity 1000 (new defs applied 134–163 times), bindings (node 10, py 12), codegen, tools/sim 149, API + evals + sim_runner + codegen 290, both golden replays pass unchanged, tsc, vitest 141, build, Playwright 24/24. WASM core 2.03 → 2.64 MB (gzip 660 → 868 KB)
- [ ] **P5.3 · library**: `ref` port + slots on op-amp templates, part generators, ~150 parts, ~60 templates, packages, BOM. Gate: the analyser buildable by hand
- [ ] **P5.4 · editor**: library panel, component editor (selection, laws, SPICE, AI), generated symbols, instances, test plans, BOM view
- [ ] **P5.5 · generation v2**: brief, architect, retrieval, slots, gap synthesis, assembly verification, test plans, deliverables; 60-prompt eval set
- [ ] **P5.6 · tutor** over instances and black boxes
- [ ] Quick wins (any time): 16k prompt limit, potentiometer and test point parts, `unsupported_request` naming the missing pieces, `MAX_BLOCKS` 16

## Scope gaps outside the phase table

| Item | v1 target (LLD 1) | Now |
| --- | --- | --- |
| Registry parts | ~150 | 15 |
| Verified block templates | ~60 | 20 |

Known smaller gaps, each noted in the LLD's "as built" sections:

- Opening a project while the API is down (only a project already open keeps working offline)
- Timeline scrub and lesson replay
- The regulator's "dropout headroom" check
- Vector retrieval: pgvector tables exist but stay empty until the registry outgrows k = 40 candidates
- Client telemetry backend (version-skew reloads are only logged)

## When you come back

1. Read the "At a glance" table and the first unchecked box above; that is the next step (now: Phase 5, P5.2 laws and value rules).
2. Check the decision noted there (now: Phase 5's decisions are taken, LLD 17; Phase 4's wait, and its deployment target is asked when that part comes).
3. After finishing a part: tick it, add the commit to the history table, update the percentage and "Last updated".
