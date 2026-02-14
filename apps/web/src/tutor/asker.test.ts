// Asking against the real core and a scripted server: edits sent first, what the request carries,
// the answer streamed into the thread, refusals, stop, feedback, and an experiment tried and undone.
import { describe, expect, it } from "vitest";
import type { AskRequest, Inserted } from "../gen/contract.ts";
import { createCircuitStore } from "../store/circuitStore.ts";
import { bundle, bundleJson, loadCore, missingArtifacts } from "../test/artifacts.ts";
import { createAsker } from "./asker.ts";
import { createTutorStore } from "./tutorStore.ts";

const missing = missingArtifacts();

const TRY = { ops: [{ op: "part.set_param", body: { refdes: "R1", key: "resistance", value: "2k" } }], predict: "fc halves" };
const REPLY = `[R1] and [C1] set the corner; [R9] is not here.\n\n\`\`\`try\n${JSON.stringify(TRY)}\n\`\`\``;

const sse = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;

function setup(reply: (req: AskRequest) => Response | string[]) {
  const core = loadCore();
  const session = new core.CoreSession(core.CoreRegistry.fromJson(bundleJson()), null);
  const store = createCircuitStore(session);
  const ins = JSON.parse(session.insertBlock(JSON.stringify({ template: "rc_lowpass" }))).ok as Inserted;
  store.getState().applyBatch(ins.ops, "Insert RC", "template");
  const tutor = createTutorStore();
  const log: string[] = [];
  const sent: AskRequest[] = [];
  const fetch = (async (_: RequestInfo | URL, init?: RequestInit) => {
    const req = JSON.parse(init!.body as string) as AskRequest;
    sent.push(req);
    log.push("ask");
    const out = reply(req);
    if (out instanceof Response) return out;
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (const chunk of out) c.enqueue(new TextEncoder().encode(chunk));
        c.close();
      },
    });
    return new Response(body, { status: 200 });
  }) as typeof globalThis.fetch;
  const api = {
    askUrl: (id: string) => `/v1/projects/${id}/ask`,
    authHeaders: async () => ({}),
    feedback: async (id: string, v: number) => void log.push(`feedback ${id} ${v}`),
    dropToken: () => void log.push("drop token"),
  };
  const asker = createAsker({
    api, project: "p1", store, tutor, registry: bundle(), fetch, simWaitMs: 50,
    flush: async () => void log.push("flush"),
  });
  /** What the server would stream: the reply word by word, then the core's reading of it. */
  const answer = (text: string) => [
    ...text.match(/\S+\s*/g)!.map((t) => sse("answer.delta", { text: t })),
    sse("answer.done", { ask_id: "a1", answer: JSON.parse(session.readAnswer(text)), usage: { in_tokens: 10, out_tokens: 5 } }),
  ];
  return { store, tutor, asker, log, sent, answer };
}

describe.skipIf(missing.length > 0)("asker", () => {
  it("sends edits first, then the question about the circuit as it is, and streams the answer into the thread", async () => {
    const t = setup(() => t.answer(REPLY));
    t.tutor.getState().setSettings({ mode: "socratic", effort: "high", level: "advanced" });
    await t.asker.ask("Why 1k?", { kind: "part", refdes: "R1" });

    expect(t.log).toEqual(["flush", "ask"]);
    const req = t.sent[0]!;
    expect(req).toMatchObject({ question: "Why 1k?", rev: t.store.getState().rev, level: "advanced", mode: "socratic", effort: "high", selection: { kind: "part", refdes: "R1" } });
    expect(req.sim).toEqual({}); // nothing simulated in this test
    const [e] = t.tutor.getState().entries;
    expect(e).toMatchObject({ phase: "done", askId: "a1", text: REPLY, mode: "socratic" });
    expect(e!.answer!.refs_valid).toBe(2);
    expect(e!.answer!.try!.problems).toEqual([]);

    await t.asker.feedback(e!.key, 1);
    expect(t.log.at(-1)).toBe("feedback a1 1");
    expect(t.tutor.getState().entries[0]!.feedback).toBe(1);
  });

  it("leaves out an effort the learner did not choose", async () => {
    const t = setup(() => t.answer("Fine."));
    await t.asker.ask("Why?", null);
    expect("effort" in t.sent[0]! || "selection" in t.sent[0]!).toBe(false);
  });

  it("shows a refusal on the question, and forgets a token the server no longer accepts", async () => {
    const t = setup(() => new Response(JSON.stringify({ code: "rev_not_synced", message: "behind", retryable: true }), { status: 409 }));
    await t.asker.ask("Why?", null);
    expect(t.tutor.getState().entries[0]).toMatchObject({ phase: "failed", error: { code: "rev_not_synced", retryable: true } });

    const u = setup(() => new Response(JSON.stringify({ code: "unauthorized", message: "expired" }), { status: 401 }));
    await u.asker.ask("Why?", null);
    expect(u.log).toContain("drop token");
  });

  it("stops an answer, keeping what arrived", async () => {
    const t = setup(() => new Response(new ReadableStream({ start: (c) => c.enqueue(new TextEncoder().encode(sse("answer.delta", { text: "Half " }))) }), { status: 200 }));
    const asking = t.asker.ask("Why?", null);
    await expect.poll(() => t.tutor.getState().entries[0]?.text).toBe("Half ");
    t.asker.stop();
    await asking;
    expect(t.tutor.getState().entries[0]).toMatchObject({ phase: "stopped", text: "Half " });
  });

  it("tries an experiment as one undo step, then undoes it", async () => {
    const t = setup(() => t.answer(REPLY));
    await t.asker.ask("How do I halve fc?", null);
    const key = t.tutor.getState().entries[0]!.key;
    const was = t.store.getState().parts.R1!.params.resistance!.display;

    expect(await t.asker.tryIt(key)).toBeNull();
    const s = t.store.getState();
    expect(s.parts.R1!.params.resistance!.display).toBe("2kΩ");
    expect(s.history.undo.at(-1)!.label).toBe("Try: fc halves");
    expect(t.tutor.getState().entries[0]!.trial).toMatchObject({ state: "measured", rev: s.rev, before: [], after: [] });

    expect(t.asker.undoTrial(key)).toBe(true);
    expect(t.store.getState().parts.R1!.params.resistance!.display).toBe(was);
    expect(t.tutor.getState().entries[0]!.trial!.state).toBe("undone");
    expect(t.asker.undoTrial(key)).toBe(false);
  });

  it("does not apply an experiment the core refused", async () => {
    const bad = { ops: [{ op: "block.remove", body: { id: "b1" } }], predict: "gone" };
    const t = setup(() => t.answer(`No.\n\n\`\`\`try\n${JSON.stringify(bad)}\n\`\`\``));
    await t.asker.ask("Remove it?", null);
    const e = t.tutor.getState().entries[0]!;
    expect(e.answer!.try!.problems[0]!.code).toBe("forbidden");
    const rev = t.store.getState().rev;
    expect(await t.asker.tryIt(e.key)).toBeNull();
    expect(t.store.getState().rev).toBe(rev);
    expect(t.tutor.getState().entries[0]!.trial).toBeUndefined();
  });
});
