# Generation evals

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
| `tests/` | The harness's own tests, on scripted replies |

## Metrics

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

## Running

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

## The gate

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

## Replies from a chat: the exchange (`exchange.py`)

Measures replies from any chat model (Gemini or DeepSeek in their web apps, Claude, a person) the way a
live run measures an API. The run hands out the exact requests the API would get, one round at a time, and
checks each reply as it would an API's: the schema, circuit-core's trial, the bench simulation and the spec
checks.

```sh
.venv/Scripts/python evals/run_evals.py --exchange evals/results/my-gemini --label "gemini-2.5-flash, AI Studio" --gate
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

## Accepting a live run

After a live run you agree with, replay it through the gate first (`--replay evals/results/<run>/cassette.json
--gate`). A call the job cancelled (a narration still waiting out a rate limit when its job finished) is in
the cassette as `cancelled`, and its replay waits to be cancelled again. If the run replays cleanly, copy its `report.json` and `cassette.json` from
`evals/results/<run>/` into `evals/golden/`. CI then replays it on every push (the "Generation evals gate" step runs once
`evals/golden/cassette.json` exists).
