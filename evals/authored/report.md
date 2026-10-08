# Generation evals: 2026-10-08T19:06:27+00:00

Provider: exchange: Claude Opus 5.5 (Claude Code), authored (evals\results\authored-exchange-2). Mode: compose. Prompts: 100 (95 in scope, 5 out of scope). Registry 2025.09.0. Wall time 59 s.

**Gate: passed.**

Phase 2 gate (LLD 16): blocks committed without template fallback ≥ 80%: 100.0% ✅

Drafting (reported, not gated): the model drafted 0.0% of blocks; n/a of the 0 blocks where it tried a draft committed as its own draft.

| Metric | Value | Target (LLD 15, 1) | |
| --- | --- | --- | --- |
| Blocks committed without template fallback | 100.0% | ≥ 90.0% | ✅ |
| First-attempt pass rate | 100.0% | ≥ 70.0% | ✅ |
| Mean attempts per block | 1.00 | ≤ 1.40 | ✅ |
| Spec error, mean relative error of the checks | 0.6% | ≤ 5.0% | ✅ |
| Input tokens per circuit (mean) | 9,350 (estimated: characters / 4) | ≤ 40,000 | – |
| Output tokens per circuit (mean) | 142 (estimated: characters / 4) | ≤ 6,000 | – |
| Full circuit latency, p95 (s) | not measured (no model time in this run) | ≤ 25.00 | – |
| First narration token, p95 (s) | not measured (no model time in this run) | ≤ 1.50 | – |
| First committed block, p95 (s) | not measured (no model time in this run) | ≤ 6.00 | – |

## Jobs

- Done: 95 of 95 in-scope prompts (100.0%); failed: none
- Out of scope refused (`unsupported_request`): 5 of 5
- Plans that use the expected templates: 100.0%; model calls per plan: 1.00
- Blocks: 137; the model's own draft: 0.0%; spec checks passed: 100.0% of 173
- Fallbacks by cause: none
- Failed attempts by error: none
- Tokens per circuit, p95: 15,404 in, 242 out (estimated: characters / 4)

## By role and level

| | Blocks | Without fallback | First attempt | Mean attempts |
| --- | --- | --- | --- | --- |
| amplifier | 28 | 100.0% | 100.0% | 1.00 |
| bias | 5 | 100.0% | 100.0% | 1.00 |
| buffer | 13 | 100.0% | 100.0% | 1.00 |
| comparator | 10 | 100.0% | 100.0% | 1.00 |
| filter | 38 | 100.0% | 100.0% | 1.00 |
| oscillator | 14 | 100.0% | 100.0% | 1.00 |
| source | 22 | 100.0% | 100.0% | 1.00 |
| supply | 7 | 100.0% | 100.0% | 1.00 |
| beginner | 44 | 100.0% | 100.0% | 1.00 |
| intermediate | 50 | 100.0% | 100.0% | 1.00 |
| advanced | 43 | 100.0% | 100.0% | 1.00 |

## Model calls

| Kind | Calls | Errors | In (mean) | Out (mean) | Cached in | p50 ms | p95 ms | Models |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| compose | 137 | 0 | 3,778 | 11 | 0 | 5 | 7 | chat:Claude Opus 5.5 (Claude Code), authored |
| narrate | 95 | 0 | 296 | 43 | 0 | 805 | 1,070 | chat:Claude Opus 5.5 (Claude Code), authored |
| plan | 110 | 0 | 3,615 | 76 | 0 | 5 | 7 | chat:Claude Opus 5.5 (Claude Code), authored |
