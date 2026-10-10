import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, expect, it, vi } from "vitest";
import { createGenerationStore, type GenerationState } from "../store/generationStore.ts";
import { GeneratePanel } from "./GeneratePanel.tsx";

const context = vi.hoisted(() => ({ gen: null as ReturnType<typeof createGenerationStore> | null, rev: 12 }));
vi.mock("./editorContext.ts", () => ({
  useEditor: () => ({ project: {}, gen: context.gen }),
  useGen: (selector: (s: GenerationState) => unknown) => selector(context.gen!.getState()),
  useCircuit: (selector: (s: { rev: number }) => unknown) => selector({ rev: context.rev }),
}));

beforeEach(() => {
  context.gen = createGenerationStore();
  context.rev = 12;
});

const summary = { rev: 12, status: "failed" as const, attempt: 2, max_attempts: 2, checks: [],
  requirements: [{ text: "Two filter stages", status: "missing" as const, evidence: "Only one is connected.", blocks: [], checks: [] }],
  problems: ["b2: cutoff outside tolerance"] };
const render = () => renderToStaticMarkup(<GeneratePanel />);

it("shows unresolved verification failures and does not offer regeneration as repair", () => {
  const s = context.gen!.getState();
  s.started("job", { prompt: "a filter" });
  s.setVerification(summary);
  s.finish("failed", { code: "assembly_verification_failed", message: "Circuit kept", retryable: true });
  const html = render();
  expect(html).toContain("Circuit built; verification failed.");
  expect(html).toContain("cutoff outside tolerance");
  expect(html).toContain("Two filter stages");
  expect(html).toContain("Only one is connected.");
  expect(html).not.toContain(">Retry<");
});

it("shows the bounded repair counter while a candidate is checked", () => {
  const s = context.gen!.getState();
  s.started("job", { prompt: "a filter" });
  s.setVerification({ ...summary, status: "repairing", attempt: 1 });
  s.setState("repairing_circuit", null);
  expect(render()).toContain("Repairing assembled circuit · attempt 1 of 2");
  s.setState("verifying_circuit", null);
  expect(render()).toContain("Checking repair · attempt 1 of 2");
});

it("restores a saved result and hides its verification claim after an edit", () => {
  context.gen!.getState().setVerification({ ...summary, status: "passed", problems: [] });
  expect(render()).toContain("Circuit checked against your request and its electrical specifications.");
  context.rev++;
  expect(render()).not.toContain("Circuit checked against your request");
});

it("never calls an incomplete measurement verified", () => {
  context.gen!.getState().setVerification({ ...summary, status: "incomplete" });
  expect(render()).toContain("verification incomplete");
  expect(render()).not.toContain("Circuit verified");
});
