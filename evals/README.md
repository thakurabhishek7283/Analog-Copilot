# Evals

Two harnesses: generation (this part) and the tutor ([Tutor evals](#tutor-evals), the Phase 3 gate).

## Generation evals

The Phase 2 gate (LLD 16) and the CI gate on prompts, models and templates (LLD 15). `run_evals.py` runs
every prompt in `prompts.yaml` as a real generation job: the real API and orchestrator, Postgres and
Redis in testcontainers, the sim_runner worker on the native ngspice (the setup of `tools/e2e/stack.py`).
It then reads each job back from its event stream and its job row. Needs Docker and the native ngspice,
like the API tests.

| File | What |
| --- | --- |
| `prompts.yaml` | 100 prompts: every template, three learner levels, single blocks, chains of 2 to 4 blocks, vague requests, prompt injection, 5 out-of-scope requests. Each lists the templates its plan should use. |
| `run_evals.py` | The runner: providers, recording and replay, the report |
| `exchange.py` | The exchange provider: replies from a chat or a person, request by request (below) |
| `metrics.py` | The metrics, the gate and the Markdown report (pure functions) |
| `golden/` | The accepted run, once there is one: `report.json` (the baseline) and `cassette.json` (its model replies) |
| `results/` | Each run's `report.json`, `report.md` and, with `--record`, `cassette.json` (not committed) |
| `tests/` | Both harnesses' own tests, on scripted replies |
| `run_tutor_evals.py`, `tutor_metrics.py`, `tutor_judge.py`, `tutor/`, `tutor_golden/` | The tutor evals (below) |

### Metrics

From each block's outcome (`jobs.plan.blocks[].outcome.how`): `draft` (the model's block passed),
`use_template` (the model kept the template; its choice, not a fallback), `fallback` (three failed
attempts, or the provider gave up).

| Metric | Target |
| --- | --- |
| Blocks committed without template fallback | ≥ 80% (Phase 2 gate), ≥ 90% (LLD 15) |
| First-attempt pass rate | ≥ 70% |
| Mean attempts per block | ≤ 1.4 |
| Spec error, mean relative error of the checks | ≤ 5% |
| Tokens per circuit, the providers' own counts | ≤ 40k in, 6k out |
| Full circuit latency, p95 | < 25 s |
| First narration token, first committed block (p95, from Generate) | < 1.5 s, < 6 s (LLD 1) |

The gate counts `use_template` as success: keeping a verified template is right when it does what was asked.
Drafting is reported next to the gate, not gated: the share of blocks the model drafted, and of the blocks
where it tried a draft (one passed, or an attempt failed), the share that committed as its own draft.

The report also has plans that miss the expected templates, out-of-scope requests not refused, failed jobs,
fallbacks with each attempt's error codes, results by role and by level, and per call kind (plan, compose,
narrate): calls, errors, tokens, cached tokens and latency.

### Running

```sh
# Scripted replies (no network). apps/api/fake/script.json gives every prompt the same circuit: a smoke run.
.venv/Scripts/python evals/run_evals.py --fake apps/api/fake/script.json --limit 10

# The live run: the provider and models in LLM_PROVIDER and its variables (apps/api/tutor_api/llm/config.py),
# recorded. A real provider is refused without --live.
.venv/Scripts/python evals/run_evals.py --env-file .env --live --record --gate

# Replay a recorded run, gated against its baseline (what CI does):
.venv/Scripts/python evals/run_evals.py --replay evals/golden/cassette.json --gate --baseline evals/golden/report.json
```

`--only id,id`, `--tags chain,scope` and `--limit n` pick prompts; `--mode templates` runs template mode;
`--concurrency` (default 4) is the number of jobs at once. `--env-file` does not override variables that
are already set.

Rate limits: a live run waits out a provider's 429s (`--max-wait`, default 900 s per call), using the delay
the provider suggests or else 30 s. An account's quota (Gemini's free tier allows 5 requests a minute to a
model; an Azure deployment has a tokens-per-minute limit) then cannot turn into template fallbacks that
would be charged to the model. A daily quota is not waited out: those calls fail `quota_exhausted` and fail
the gate. Since waits count toward a job's time, live runs allow 3600 s per job (`--job-timeout`). When any
wait happened, the report leaves out job latency and keeps the per-call model latency (Model calls table).
On a rate-limited account, use `--concurrency 2`.

### The gate

`--gate` exits 1 when:

- fewer than 80% of blocks commit without template fallback (`--min-commit` changes the floor);
- with `--baseline`: the commit rate is more than 2 points below the baseline's, or input or output tokens
  per circuit are more than 15% above it;
- a replay asks for a model call the cassette does not hold;
- an exchange run still has calls waiting for a reply.

Only measured token counts compare against a baseline: when either run's tokens are estimated (scripted or
exchange replies), the token rule is skipped. Latency is reported for live runs only.

Replaying is deterministic: the same cassette gives the same plans, blocks, outcomes and tokens (tested in
`tests/test_run_evals.py`). So in CI the replay checks that nothing changed what the pipeline asks the model.
A change to a prompt, a template, the registry or the orchestrator changes the requests, and the replay fails
with "not in the cassette". The change then needs a new live run, which is the real measurement.

### Replies from a chat: the exchange (`exchange.py`)

Measures replies from any chat model (Gemini or DeepSeek in their web apps, Claude, a person) the way a
live run measures an API. The run hands out the exact requests the API would get, one round at a time, and
checks each reply as it would an API's: the schema, circuit-core's trial, the bench simulation and the spec
checks.

```sh
.venv/Scripts/python evals/run_evals.py --exchange evals/results/my-gemini --label "gemini-3.5-flash, AI Studio" --gate
```

1. The run stops every job at its first unanswered model call and writes that call's prompt to
   `evals/results/my-gemini/pending/<name>.txt`. The first round is the 100 plans.
2. Paste each prompt into the chat. If the chat has a system-instructions field, the part under SYSTEM goes
   there. Save the reply exactly as the chat gave it to `replies/<name>.txt`, using the same name.
   `python evals/exchange.py evals/results/my-gemini` shows what is still waiting.
3. Run the same command again. Answered calls replay, and each job goes on to its next call: compose after
   plan, a repair after a failed draft (quoting the problem and the measured value), the narration. The
   run exits 3 while calls are waiting, and with the gate's verdict once none are.

Each round reruns every prompt (about a minute). A chain of four blocks takes about six rounds. The
prompts are the messages a JSON-mode endpoint gets (the schema is appended to the user turn), so a chat
reply corresponds to an API reply. One difference: a chat does not enforce the schema, so a reply in a
Markdown fence is accepted and anything else that is not the JSON is a `schema_error` attempt, as from an
API.

To keep the run a measurement:
- A reply is frozen once a run has used it (`ledger.json`). If it changes, the run refuses to start.
  Answer again in a new directory.
- Answer each prompt from what it shows, in a fresh chat per prompt if the chat carries history: a model
  does not learn from other prompts' results.
- Token counts are estimated (characters / 4) and latency is not measured. Measure both with a live run.

Add `--record` to the last round to keep its cassette. That is a recording like a live run's, and it can be
replayed or accepted into `golden/`.

### Accepting a live run

After a live run you agree with, replay it through the gate first (`--replay evals/results/<run>/cassette.json
--gate`). A call the job cancelled (a narration still waiting out a rate limit when its job finished) is in
the cassette as `cancelled`, and its replay waits to be cancelled again. If the run replays cleanly, copy its `report.json` and `cassette.json` from
`evals/results/<run>/` into `evals/golden/`. CI then replays it on every push (the "Generation evals gate" step runs once
`evals/golden/cassette.json` exists).

## Tutor evals

The Phase 3 gate (LLD 16: ≥ 99% valid references) and the tutor's CI gate (LLD 15). `run_tutor_evals.py`
asks every case in `tutor/cases.yaml` through the real API (Postgres and Redis in testcontainers): it
builds the case's circuit from its ops, sends `POST /ask` or `POST /what-changed` with the simulation
values the editor would send, and reads the answer from the stream. Then it checks each answer and,
with `--rubric`, has a judge grade a sample. Needs Docker and the native ngspice.

| File | What |
| --- | --- |
| `tutor/circuits.yaml` | 26 circuits as a student builds them (Insert block, or parts and wires): every template in a bench with a source, two chains, two from parts, and four broken ones (a detuned filter, an unwired feedback resistor, not simulated yet, shorted sources) |
| `tutor/cases.yaml` | 130 cases: 100 questions (why a value, what a part does, change a spec, predict an edit, read the simulation, debug, missing data, a wrong premise, off topic and injection, and 10 follow-ups whose earlier `turns` are asked first and sent as the question's history, or that come `after` a "What changed?") and 30 "What changed?" edits (a check that fails or recovers, tweaks, swaps, parts added or removed, rewiring, a drag, an analysis change, several edits). Three levels, 28 in Socratic mode, 60 tagged `rubric` (45 + 15) |
| `tutor/fixtures.py`, `tutor/sim_values.mts` | Build the circuits and edits with circuit-core and simulate each the way the editor does (its own store, scheduler and `simValues`, on ngspice.wasm in Node), frozen into `tutor/fixtures.json` |
| `run_tutor_evals.py` | The runner: providers, recording and replay, the report |
| `tutor_metrics.py` | The metrics, numeric grounding, the gate, the report; `compare` and `agreement` commands |
| `tutor_judge.py` | The LLM rubric |
| `tutor_golden/` | The accepted run, once there is one: `report.json` and `cassette.json` |

Frozen values make every run send the model the same text on any OS, so a recording replays in CI
(the generation evals learned this from a threshold that read differently on Linux ngspice). After a
change to the circuits, the cases' edits, the registry or the editor's simulation, rebuild them:
`.venv/Scripts/python evals/tutor/fixtures.py` (needs `node`, the WASM core and ngspice.wasm, as the
web tests do). `tests/test_tutor_fixtures.py` fails until then.

### Tutor metrics

| Metric | How | Target |
| --- | --- | --- |
| Valid references | circuit-core's reading of each answer (`refs_valid` / all). A "What changed?" answer is read against the circuit after the edit, so a part the edit removed is not in it; one the change summary names counts as valid here (the editor shows it as plain text) | ≥ 99% (gated) |
| Numeric grounding | every quantity in the answer's body (LaTeX read as the plain text it means): in the context, the question (with a follow-up's earlier turns), a `target ±tol%` band edge, or the answer's own experiment, at the precision shown (rounded by at most 5%); the result of arithmetic the answer shows, before (`= 884 Hz`) or after it (`6.83 V (12 V − 5.17 V)`), evaluated when numeric (an error is ungrounded); a textbook constant (2π, 0.707, −3 dB, 90°, 0.7 V, 26 mV); or part of a name the context uses (the 555 of NE555). The rest are ungrounded and listed: those one step of arithmetic from the context (996 Hz − 956 Hz = 40 Hz, not shown) apart from those from nowhere. Reported as answers fully grounded, and grounded or only one step short | vs baseline |
| Experiments | the `try` block validated by circuit-core; a valid one is applied to the circuit and both are simulated on the native ngspice: each number in the prediction against the checks of its unit that the experiment moved, a source's own checks left out (held when one agrees within 10% or the check's tolerance; not judged when nothing comparable moved) | vs baseline |
| Behaviour | expected citations made, an experiment when one is expected and none when not, words (≤ 150), Socratic answers that ask a question (follow-ups left out), beginner answers with jargon (ERC, operating point, DC bias, quiescent, pin names like C1.2, LaTeX, a list of phase angles) | report |
| Rubric | the judge scores correct, grounded, answers, level, mode and teaching from 1 to 5, with a pass or fail; for a follow-up it also sees the earlier turns, and mode includes confirming or correcting the student's reply first | report |

Also: references valid in the circuit but outside the slice, arithmetic errors, tokens (cached
included), latency to the first token and to the whole answer, and every metric by kind, category,
level and mode.

The gate (`--gate`) fails a run whose references are below 99%, with more than 2% of cases unanswered,
or with a cassette miss; with `--baseline`, when valid references drop more than 1 point, grounded
answers more than 3, valid experiments more than 5, or tokens per answer rise more than 15%. Grounding
is gated against the baseline only until a live run shows how often the checker is wrong.

### Running the tutor evals

```sh
# Scripted replies (no network): a smoke run of the harness.
.venv/Scripts/python evals/run_tutor_evals.py --fake SCRIPT.json --limit 10

# Live, recorded, graded: one run per setting to compare (the learner's Quick and Deep thinking).
.venv/Scripts/python evals/run_tutor_evals.py --env-file .env --live --record --rubric --effort low
.venv/Scripts/python evals/run_tutor_evals.py --env-file .env --live --record --rubric --effort high
.venv/Scripts/python evals/tutor_metrics.py compare evals/results/<run-a> evals/results/<run-b>

# What CI does: replay the accepted run (its tiers, effort and rubric come from the baseline).
.venv/Scripts/python evals/run_tutor_evals.py --replay evals/tutor_golden/cassette.json --gate --baseline evals/tutor_golden/report.json
```

`--tier small|large` answers both kinds on that tier (default: `LLM_TIER_ASK`, `LLM_TIER_WHAT_CHANGED`);
comparing tiers means something only when they name different models. `--kind`, `--only`, `--tags`
and `--limit` pick cases. `--exchange DIR` works as for generation: a chat or a person answers.

The judge (`--rubric`) grades the cases tagged `rubric`. With `--live` it is `--judge` (default
`deepseek`: another model family than the tutor's, so it does not grade its own style); otherwise it
answers from the same script, cassette or exchange as the tutor. Its calls go into the run's cassette,
so a replay grades the same way. For that the judge's request holds only facts that are the same on
every machine and every version of these checkers: the answer, its input, and circuit-core's reading
of its references and experiment (not the re-simulation, which differs slightly between Windows and
Linux ngspice). `--judge-live` regrades a recorded run: the tutor's replies replay, the judge is live.
Each graded run writes `review.csv`: the sample with the judge's scores
and empty columns for a human reviewer's on the same scale (LLD 15: "plus one human reviewer");
`python evals/tutor_metrics.py agreement review.csv` then reports how often the verdicts agree and how
far the scores are apart.

To accept a live run, replay it through the gate first, then copy its `report.json` and `cassette.json`
into `evals/tutor_golden/`; the "Tutor evals gate" CI step replays it on every push.
