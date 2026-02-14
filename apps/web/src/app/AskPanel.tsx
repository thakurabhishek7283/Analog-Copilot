// Ask (LLD §9, §10): questions about the circuit, answered from its own values. The selection is
// what a question is about (click a part, then ask). Answers stream; their references are chips
// that select what they name, and one the circuit does not hold is plain text. An experiment is a
// card: its ops, the tutor's prediction, Try it (one undo step), then what the simulation measured.
// Text is rendered as text, never as HTML (LLD §14).
import { useEffect, useMemo, useRef, useState } from "react";
import type { Answer, LearnerLevel, ReasoningEffort, Ref, Selection } from "../gen/contract.ts";
import { changedChecks, describeOp, segments } from "../tutor/answer.ts";
import type { AskEntry } from "../tutor/tutorStore.ts";
import { useCircuit, useEditor, useTutor } from "./editorContext.ts";

const MAX_QUESTION = 1000;

const ERROR_TEXT: Record<string, string> = {
  offline: "The server cannot be reached.",
  tutor_unavailable: "The tutor is not available on this server right now.",
  llm_unavailable: "The model is not answering.",
  tutor_timeout: "The answer took too long.",
  stream_lost: "The answer was cut off.",
  rev_not_synced: "Your latest edits were not saved yet.",
  stale_rev: "The circuit changed on the server.",
  part_not_found: "That part is not in the saved circuit.",
  net_not_found: "That net is not in the saved circuit.",
  block_not_found: "That block is not in the saved circuit.",
};

const EFFORTS: { value: ReasoningEffort | null; label: string; title: string }[] = [
  { value: null, label: "Normal", title: "The model's usual amount of thinking" },
  { value: "low", label: "Quick", title: "Less thinking: a faster answer" },
  { value: "high", label: "Deep", title: "More thinking: slower, better at working things out" },
];

function useSelectionName(sel: Selection | null): string | null {
  const name = useCircuit((s) => {
    if (!sel) return null;
    if (sel.kind === "part") return s.parts[sel.refdes] ? sel.refdes : null;
    if (sel.kind === "net") return s.nets[sel.id] ? (s.nets[sel.id]!.label ?? sel.id) : null;
    return s.blocks[sel.id]?.title ?? null;
  });
  return name;
}

export function AskPanel() {
  const { project, tutor } = useEditor();
  const entries = useTutor((s) => s.entries);
  const settings = useTutor((s) => s.settings);
  const selection = useCircuit((s) => s.selection);
  const selectionName = useSelectionName(selection);
  const [question, setQuestion] = useState("");
  // A question is about the selection unless the learner takes it off (until the next selection).
  const [about, setAbout] = useState(true);
  useEffect(() => setAbout(true), [selection]);
  const busy = entries.some((e) => e.phase === "asking" || e.phase === "streaming");
  const end = useRef<HTMLDivElement>(null);
  const last = entries.at(-1);

  useEffect(() => {
    end.current?.scrollIntoView?.({ block: "nearest" });
  }, [entries.length, last?.text.length, last?.phase, last?.trial?.state, last?.feedback]);

  const asker = project?.asker;
  const target = about && selectionName ? selection : null;
  const submit = (e?: React.FormEvent) => {
    e?.preventDefault();
    const text = question.trim();
    if (!text || !asker) return;
    setQuestion("");
    void asker.ask(text, target);
  };

  return (
    <section className="ask" aria-label="Ask the tutor">
      <h3>Ask the tutor</h3>
      {entries.length > 0 && (
        <div className="ask-thread">
          {entries.map((e) => (
            <Entry key={e.key} entry={e} />
          ))}
          <div ref={end} />
        </div>
      )}
      {!asker ? (
        <p className="hint">Asking needs a saved project: press New.</p>
      ) : (
        <form onSubmit={submit}>
          {target && selectionName && (
            <span className="about">
              About <strong>{selectionName}</strong>
              <button type="button" className="link" aria-label={`Ask without ${selectionName}`} onClick={() => setAbout(false)}>
                ×
              </button>
            </span>
          )}
          <textarea
            aria-label="Your question"
            rows={2}
            maxLength={MAX_QUESTION}
            placeholder={target && selectionName ? `Ask about ${selectionName}, e.g. why is it this value?` : "Ask about this circuit, or click a part first"}
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) submit(e);
            }}
          />
          <div className="ask-row">
            <span className="segmented" role="group" aria-label="How the tutor answers">
              <button type="button" aria-pressed={settings.mode === "explain"} onClick={() => tutor.getState().setSettings({ mode: "explain" })}>
                Explain
              </button>
              <button
                type="button"
                aria-pressed={settings.mode === "socratic"}
                title="One guiding question first, so you work it out"
                onClick={() => tutor.getState().setSettings({ mode: "socratic" })}
              >
                Guide me
              </button>
            </span>
            <select
              aria-label="Thinking"
              title={EFFORTS.find((x) => x.value === settings.effort)?.title}
              value={settings.effort ?? ""}
              onChange={(e) => tutor.getState().setSettings({ effort: (e.target.value || null) as ReasoningEffort | null })}
            >
              {EFFORTS.map((x) => (
                <option key={x.label} value={x.value ?? ""}>
                  {x.label} thinking
                </option>
              ))}
            </select>
            <select aria-label="Answer level" value={settings.level} onChange={(e) => tutor.getState().setSettings({ level: e.target.value as LearnerLevel })}>
              <option value="beginner">Beginner</option>
              <option value="intermediate">Intermediate</option>
              <option value="advanced">Advanced</option>
            </select>
            <span className="spacer" />
            {busy ? (
              <button type="button" onClick={() => asker.stop()}>
                Stop
              </button>
            ) : (
              <button type="submit" className="primary" disabled={!question.trim()}>
                Ask
              </button>
            )}
          </div>
        </form>
      )}
    </section>
  );
}

function Entry({ entry }: { entry: AskEntry }) {
  const { readAnswer, project } = useEditor();
  // While it streams, the browser's core reads it: chips for references, a `try` block hidden
  // until it is complete. The server's reading (against the asked rev) replaces it at the end.
  const live = useMemo(() => (entry.answer || !entry.text ? null : readAnswer(entry.text)), [entry.answer, entry.text, readAnswer]);
  const answer: Answer | null = entry.answer ?? live;
  const aboutName = entry.selection ? (entry.selection.kind === "part" ? entry.selection.refdes : entry.selection.id) : null;
  return (
    <article className="ask-entry" data-phase={entry.phase}>
      <p className="question">
        {entry.question}
        {aboutName && <span className="about-tag"> · about {aboutName}</span>}
      </p>
      {entry.phase === "asking" && <p className="muted">Thinking…</p>}
      {answer && (
        <p className="answer-text" aria-live={entry.phase === "streaming" ? "polite" : undefined}>
          <AnswerText body={answer.body} refs={answer.refs} />
          {entry.phase === "streaming" && <span className="caret" aria-hidden="true" />}
        </p>
      )}
      {entry.phase === "stopped" && <p className="muted">Stopped.</p>}
      {entry.phase === "failed" && entry.error && (
        <p className="ask-error" title={entry.error.message}>
          {ERROR_TEXT[entry.error.code] ?? entry.error.message}{" "}
          {entry.error.retryable && project && (
            <button type="button" className="link" onClick={() => void project.asker.ask(entry.question, entry.selection)}>
              Ask again
            </button>
          )}
        </p>
      )}
      {entry.phase === "done" && entry.answer?.try && <TryCard entry={entry} />}
      {entry.phase === "done" && entry.askId && <Feedback entry={entry} />}
    </article>
  );
}

function AnswerText({ body, refs }: { body: string; refs: Ref[] }) {
  return (
    <>
      {segments(body, refs).map((s, i) => ("ref" in s ? <RefChip key={i} r={s.ref} /> : <span key={i}>{s.text}</span>))}
    </>
  );
}

/** A reference as a chip that selects what it names (while the circuit still holds it). */
function RefChip({ r }: { r: Ref }) {
  const { store } = useEditor();
  const label = useCircuit((s) => {
    if (r.kind === "part") return s.parts[r.id] ? r.id : null;
    if (r.kind === "net") return s.nets[r.id] ? (s.nets[r.id]!.label ?? r.id) : null;
    return s.blocks[r.id]?.title ?? null;
  });
  const target: Selection = r.kind === "part" ? { kind: "part", refdes: r.id } : { kind: r.kind, id: r.id };
  return (
    <button
      type="button"
      className="ref-chip"
      data-ref-kind={r.kind}
      data-ref={r.id}
      disabled={label === null}
      title={label === null ? `${r.id} is no longer in the circuit` : `Select ${label}`}
      onClick={() => store.getState().select(target)}
    >
      {label ?? r.id}
    </button>
  );
}

function TryCard({ entry }: { entry: AskEntry }) {
  const { project } = useEditor();
  const suggestion = entry.answer!.try!;
  const trial = entry.trial;
  const generating = useCircuit((s) => s.mode === "generating");
  const undoable = useCircuit((s) => !!trial && trial.state !== "undone" && s.rev === trial.rev && s.history.undo.at(-1)?.label === trial.label);
  const [refused, setRefused] = useState<string | null>(null);
  const changes = trial?.after ? changedChecks(trial.before, trial.after) : [];
  const blocked = suggestion.problems.length > 0;

  return (
    <div className="try-card" data-state={trial?.state ?? (blocked ? "blocked" : "ready")}>
      <h4>Experiment</h4>
      <ul className="ops">
        {suggestion.ops.map((op, i) => (
          <li key={i}>{describeOp(op)}</li>
        ))}
      </ul>
      {suggestion.predict && (
        <p className="predict">
          <span className="muted">Prediction:</span> {suggestion.predict}
        </p>
      )}
      {blocked && (
        <p className="ask-error">This experiment cannot be applied: {suggestion.problems.map((p) => p.message).join("; ")}</p>
      )}
      {trial?.state === "applied" && <p className="muted">Simulating…</p>}
      {trial?.state !== "applied" && trial?.after && (
        changes.length ? (
          <table className="measured" aria-label="Measured">
            <tbody>
              {changes.map((c) => (
                <tr key={`${c.block}:${c.name}`}>
                  <td>{c.label}</td>
                  <td className="num">{c.before}</td>
                  <td aria-hidden="true">→</td>
                  <td className="num now">{c.after}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="muted">No spec check changed: compare the scope and the values on the schematic.</p>
        )
      )}
      {trial?.state === "undone" && <p className="muted">Undone.</p>}
      {refused && <p className="ask-error">{refused}</p>}
      <div className="row-actions">
        {!trial && (
          <button
            type="button"
            className="primary"
            disabled={blocked || generating || !project}
            onClick={async () => {
              const err = await project!.asker.tryIt(entry.key);
              setRefused(err ? `Could not apply it: ${err.message}` : null);
            }}
          >
            Try it
          </button>
        )}
        {trial && trial.state !== "undone" && (
          <button type="button" disabled={!undoable || generating} title={undoable ? undefined : "Other changes came after it: use Undo in the toolbar"} onClick={() => project?.asker.undoTrial(entry.key)}>
            Undo it
          </button>
        )}
      </div>
    </div>
  );
}

function Feedback({ entry }: { entry: AskEntry }) {
  const { project } = useEditor();
  return (
    <div className="feedback" role="group" aria-label="Was this answer helpful?">
      <span className="muted">Helpful?</span>
      <button type="button" aria-pressed={entry.feedback === 1} onClick={() => void project?.asker.feedback(entry.key, 1)}>
        Yes
      </button>
      <button type="button" aria-pressed={entry.feedback === -1} onClick={() => void project?.asker.feedback(entry.key, -1)}>
        No
      </button>
    </div>
  );
}
