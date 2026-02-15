// One question's answer (LLD §5, §9): `POST /v1/projects/{id}/ask` (or `what-changed`, which
// answers the same way) over fetch with the bearer token,
// read as SSE: `answer.delta` while the model writes, then `answer.done` or `error`. Not resumable
// (decided 2026-02-14): a stream that drops is asked again. A refusal before the stream starts is
// an ordinary error response, thrown as an `ApiFailure`.
import { ApiFailure } from "../api/client.ts";
import type { AskEvent, AskRequest, ChangeRequest } from "../gen/contract.ts";
import { SseParser } from "../stream/sse.ts";

const EVENTS: ReadonlySet<string> = new Set(["answer.delta", "answer.done", "error"]);

export interface AskStreamOptions {
  url: string;
  headers: () => Promise<Record<string, string>>;
  body: AskRequest | ChangeRequest;
  onEvent: (event: AskEvent) => void;
  /** Aborting stops reading and cancels the answer on the server. */
  signal?: AbortSignal;
  fetch?: typeof fetch;
}

/** Resolves after `answer.done` or `error`. Rejects with an `ApiFailure`: the server's refusal,
 * `offline`, or `stream_lost` (retryable) when the stream ends without its last event. An abort
 * rejects with the signal's reason. */
export async function askStream(opts: AskStreamOptions): Promise<void> {
  const fetchFn = opts.fetch ?? ((...args) => fetch(...args));
  let res: Response;
  try {
    res = await fetchFn(opts.url, {
      method: "POST",
      headers: { Accept: "text/event-stream", "Content-Type": "application/json", ...(await opts.headers()) },
      body: JSON.stringify(opts.body),
      signal: opts.signal,
      cache: "no-store",
    });
  } catch (e) {
    if (opts.signal?.aborted) throw e;
    throw new ApiFailure(0, { code: "offline", message: `cannot reach the server (${(e as Error).message})`, retryable: true });
  }
  if (!res.ok || !res.body) {
    let error = { code: "http_error", message: `ask: HTTP ${res.status}`, retryable: false };
    try {
      const body = (await res.json()) as typeof error;
      if (typeof body?.code === "string" && typeof body.message === "string") error = body;
      else if (res.status >= 500) error = { code: "offline", message: `the server is not reachable (HTTP ${res.status})`, retryable: true };
    } catch {
      // Not the API's error body: a proxy in front of an API that is down.
      if (res.status >= 500) error = { code: "offline", message: `the server is not reachable (HTTP ${res.status})`, retryable: true };
    }
    throw new ApiFailure(res.status, error);
  }
  const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
  const parser = new SseParser();
  // Stop reading on abort ourselves, whether or not the fetch implementation errors the body.
  const stop = () => void reader.cancel().catch(() => undefined);
  opts.signal?.addEventListener("abort", stop, { once: true });
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (opts.signal?.aborted) throw opts.signal.reason;
      if (done) break;
      for (const e of parser.feed(value)) {
        if (!EVENTS.has(e.event)) {
          if (e.event !== "message") console.warn(`ask stream: unknown event ${e.event}`); // additive within a version
          continue;
        }
        const event = { event: e.event, data: JSON.parse(e.data) } as AskEvent;
        opts.onEvent(event);
        if (event.event !== "answer.delta") return;
      }
    }
  } catch (e) {
    if (opts.signal?.aborted) throw e;
    throw new ApiFailure(0, { code: "stream_lost", message: `the answer was cut off (${(e as Error).message})`, retryable: true });
  } finally {
    opts.signal?.removeEventListener("abort", stop);
    await reader.cancel().catch(() => undefined);
  }
  throw new ApiFailure(0, { code: "stream_lost", message: "the answer was cut off", retryable: true });
}
