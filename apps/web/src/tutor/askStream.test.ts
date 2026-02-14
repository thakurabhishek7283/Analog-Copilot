// The Ask stream against a scripted server: events split anywhere, refusals before the stream,
// a proxy's 5xx, a stream cut off, and a stop.
import { describe, expect, it } from "vitest";
import { ApiFailure } from "../api/client.ts";
import type { AskEvent } from "../gen/contract.ts";
import { askStream } from "./askStream.ts";

const ev = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const DONE = { ask_id: "a1", answer: { body: "Hi.", refs: [], refs_valid: 0, refs_invalid: 0 }, usage: { in_tokens: 1, out_tokens: 1 } };

function server(reply: { status?: number; body?: string; chunks?: string[]; hang?: boolean }) {
  const sent: { body: unknown; headers: Record<string, string> }[] = [];
  const fetch = async (_: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    sent.push({ body: JSON.parse(init!.body as string), headers: init!.headers as Record<string, string> });
    if (reply.status && reply.status !== 200) return new Response(reply.body ?? "", { status: reply.status });
    const signal = init?.signal;
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (const chunk of reply.chunks ?? []) c.enqueue(new TextEncoder().encode(chunk));
        if (!reply.hang) c.close();
        signal?.addEventListener("abort", () => c.error(signal.reason));
      },
    });
    return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
  };
  return { fetch: fetch as typeof globalThis.fetch, sent };
}

const opts = (fetch: typeof globalThis.fetch, events: AskEvent[], signal?: AbortSignal) => ({
  url: "/v1/projects/p/ask",
  headers: async () => ({ Authorization: "Bearer t" }),
  body: { question: "Why?", rev: 3 },
  onEvent: (e: AskEvent) => events.push(e),
  signal,
  fetch,
});

describe("askStream", () => {
  it("posts the question and reads deltas to the end, however the bytes are split", async () => {
    const text = `: ping\n\n${ev("answer.delta", { text: "Hi" })}${ev("answer.delta", { text: "." })}${ev("answer.done", DONE)}`;
    const chunks = text.match(/[\s\S]{1,5}/g)!;
    const { fetch, sent } = server({ chunks });
    const events: AskEvent[] = [];
    await askStream(opts(fetch, events));
    expect(sent[0]!.body).toEqual({ question: "Why?", rev: 3 });
    expect(sent[0]!.headers.Authorization).toBe("Bearer t");
    expect(events.map((e) => e.event)).toEqual(["answer.delta", "answer.delta", "answer.done"]);
    expect(events[2]).toEqual({ event: "answer.done", data: DONE });
  });

  it("ends at an error event", async () => {
    const { fetch } = server({ chunks: [ev("answer.delta", { text: "Hi" }), ev("error", { code: "llm_unavailable", message: "down", retryable: true })] });
    const events: AskEvent[] = [];
    await askStream(opts(fetch, events));
    expect(events.at(-1)).toEqual({ event: "error", data: { code: "llm_unavailable", message: "down", retryable: true } });
  });

  it("throws the server's refusal, and treats a proxy's 5xx as offline", async () => {
    const refusal = server({ status: 409, body: JSON.stringify({ code: "rev_not_synced", message: "behind", retryable: true }) });
    await expect(askStream(opts(refusal.fetch, []))).rejects.toMatchObject({ status: 409, error: { code: "rev_not_synced" } });
    const proxy = server({ status: 502, body: "<html>Bad gateway</html>" });
    await expect(askStream(opts(proxy.fetch, []))).rejects.toMatchObject({ error: { code: "offline", retryable: true } });
    const unreachable = (async () => {
      throw new TypeError("connection refused");
    }) as unknown as typeof globalThis.fetch;
    await expect(askStream(opts(unreachable, []))).rejects.toMatchObject({ error: { code: "offline" } });
  });

  it("reports a stream that ends without its last event as cut off", async () => {
    const { fetch } = server({ chunks: [ev("answer.delta", { text: "Hal" })] });
    const err = await askStream(opts(fetch, [])).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiFailure);
    expect((err as ApiFailure).error).toMatchObject({ code: "stream_lost", retryable: true });
  });

  it("stops when aborted", async () => {
    const { fetch } = server({ chunks: [ev("answer.delta", { text: "Hal" })], hang: true });
    const stop = new AbortController();
    const events: AskEvent[] = [];
    const done = askStream(opts(fetch, events, stop.signal));
    await new Promise((r) => setTimeout(r, 10));
    stop.abort();
    await expect(done).rejects.not.toBeInstanceOf(ApiFailure);
    expect(events).toHaveLength(1);
  });
});
