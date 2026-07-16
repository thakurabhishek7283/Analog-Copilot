# Student test sessions (the Phase 3 gate)

LLD 16 closes Phase 3 on "≥ 99% valid refs; 20 student test sessions". The tutor evals (`evals/README.md`)
measure the first part on authored questions. These sessions measure what they cannot: whether real
students, asking their own questions in their own words, get answers that are right, grounded and
useful. One session is one student for about 45 minutes, with a facilitator who watches and takes
notes but does not teach.

| File | What |
| --- | --- |
| `README.md` | This guide: setup, the session script, what is logged, how to read the results |
| `tasks.md` | The student's task card (print one per session) |
| `answer_key.md` | The facilitator's key: what a right answer to each task contains, what a wrong one looks like, what the screen should show (never shown to the student) |
| `observation.md` | The facilitator's sheet (one per session) |
| `survey.md` | The questions after the session |
| `export.py` | Reads every answer back from the database with the evals' checks: `answers.csv` (a follow-up's `follows` column names the earlier questions it was sent with), `sessions.md` |

## Who

Students who have done, or are doing, a first course in circuits (Ohm's law, capacitors, an op-amp
seen once). Aim for a mix: about 8 first-years, 8 second-years and 4 further along, so all three levels
get used. Nobody who has used Analog-Copilot before, and nobody who built it.

## Setup (once)

The databases must outlive the API process, so every session's answers are still there to export:

```sh
docker compose up -d postgres redis      # compose's Postgres keeps its data in a volume; builds nothing
.venv/Scripts/python tools/e2e/stack.py --port 8000 --llm-from .env \
    --database-url postgresql+asyncpg://tutor:tutor@127.0.0.1:5432/tutor --redis-url redis://127.0.0.1:6379/0
cd apps/web && npm run build && npx vite preview      # the app at http://localhost:4173, /v1 proxied to :8000
```

`.env` selects the production model. Every question is a paid call: a session
asks about 15–25, so 20 sessions are about 400 answers. Check that the stack answers a question
before the first student arrives.

## Each session

1. **Before.** A fresh browser profile (Edge: a new guest window), so the student is a new anonymous user
   with no history; `export.py` tells sessions apart by that user. Note the start time on the sheet.
   Set the Ask panel to its defaults (Explain, Normal thinking, Beginner) and leave the level for the
   student to change.
2. **Consent (2 min).** Explain: we are testing the tutor, not them; their questions and the tutor's
   answers are stored with no name; they can stop at any time. Ask them to think aloud.
3. **Tour (3 min).** Show only: Insert block, selecting a part, the Ask panel (About chip, Explain /
   Guide me, the thinking choice), Yes/No under an answer. Do not show What changed? or Try it; the
   tasks lead there.
4. **Tasks (30 min).** Hand over `tasks.md`. Do not answer circuit questions: "what would you ask the
   tutor?" is the only hint. Keep `answer_key.md` beside you to tell whether the tutor is on track. Fill in `observation.md` as you go. Task 5 needs you at the keyboard for
   10 seconds: while the student looks away, disconnect C1 from the filter's output (select C1, and in
   the inspector disconnect its pin 2) and wait for the simulation: both checks fail (Q falls to about 0.1).
5. **Survey (5 min).** `survey.md`, on paper or read aloud.
6. **After.** Note the end time. Export (below) at the end of the day at least.

## What is logged

Every answer is a row in `asks`: the question, what was selected, the kind (a question or What
changed?), mode, level, thinking effort, the context the model was given, the whole answer, its valid
and invalid references, tokens, time to the first word and to the end, and the student's Yes/No. Every
edit is in the op log, including a Try it they applied. Nothing records names; keep the sheets'
session numbers and the export's `S01`… in the same order (by start time).

```sh
.venv/Scripts/python evals/sessions/export.py --since 2026-03-11T09:00 --until 2026-03-11T18:00
```

writes `evals/sessions/results/answers.csv` (every answer, for reading) and `sessions.md` (per session
and in all). Both stay out of git (students' words).

## Reading the results

For each session, read every answer in `answers.csv` next to the sheet and `answer_key.md`. Mark each answer **ok**, **weak**
(right but unhelpful: too long, wrong level, did not answer what was asked) or **wrong** (a false claim
about the circuit, a wrong number, an invented part). A wrong answer is the finding that matters most;
note whether the student noticed it.

`sessions.md` gives the automatic side, the same checks as the tutor evals: references that name
nothing in the circuit, numbers not found in the context nor derived with arithmetic shown (read each;
the checker is strict), experiments tried, and Yes/No.

Phase 3 passes when, over the 20 sessions:

| Criterion | Target |
| --- | --- |
| Valid references (`sessions.md`, All) | ≥ 99% |
| Answers marked wrong by the reviewer | ≤ 2%, and none that misled a student unnoticed |
| Answers the students rated helpful (of those rated) | ≥ 75% |
| Students who finished tasks 1–4 without a hint from the facilitator | ≥ 16 of 20 |
| Survey: "I trusted the tutor's numbers" (agree or strongly agree) | ≥ 70% |

Whatever the totals, list the issues by severity (a wrong answer, a confusing one, a missing feature, a
UI problem) with the session and time, and decide which to fix before Phase 3 closes.
