"""Model replies from a person or a chat session instead of an API: the exchange provider.

An eval run with `--exchange DIR` asks this provider for every model call. A call with a reply in
`DIR/replies/` is answered with it. A call without one is written to `DIR/pending/` as a paste-ready
prompt and stops its job (a narration stops only itself). Whoever answers (a chat with Gemini,
DeepSeek or Claude, or a person) saves each reply, exactly as given, under the same file name in
`DIR/replies/`, and runs the same command again. Answered calls replay, the pipeline reaches its
next requests (compose after plan, a repair after a failed draft, with the measured values), and the run
is complete when nothing is pending.

Replies go through the same checks as an API's: the schema, circuit-core's trial, the bench simulation
and the spec checks. So a run measures the answers the way a live run measures a provider's. A reply is
frozen once a run has used it (`ledger.json`): changing it afterwards would be tuning on the results, and
the provider refuses to start. Start a new directory to answer again.

Not measured: token counts (estimated as characters / 4, and marked estimated) and latency (there is no
model time). A reply longer than the call's max_tokens counts as cut off, as an API would cut it.

    python evals/exchange.py DIR      # what is pending and answered
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from tutor_api.llm.base import LlmRequest, LlmResponse, OnDelta, Usage  # noqa: E402
from tutor_api.llm.openai_compat import OpenAICompatProvider  # noqa: E402

# The messages exactly as a json_object endpoint (DeepSeek on Azure) gets them: the schema appended to
# the user turn. A chat has no response_format, so this is the form that fits a paste.
_SHAPER = OpenAICompatProvider("exchange", "http://unused", "unused", {"large": "large", "small": "small"},
                               json_mode="json_object")


class AwaitingReply(Exception):
    """The call has no reply yet; its prompt is in pending/. Not a ProviderError, so no retry or
    fallback hides it: the job stops and the run is incomplete."""


class ReplyChanged(Exception):
    """A reply a run already used was changed."""


def stem(req: LlmRequest) -> str:
    return f"{req.kind}-{req.key()[:12]}"


def sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def prompt_text(req: LlmRequest, name: str) -> str:
    """What to paste into a chat, after a header that says where the reply goes."""
    body = _SHAPER.body(req, False)
    system, user = (m["content"] for m in body["messages"])
    tier = "large model" if req.tier == "large" else "small model"
    shape = "one JSON object" if req.schema is not None else "plain text"
    return (
        f"# Analog-Copilot model call: {req.kind} ({tier}), temperature {req.temperature:g}, at most {req.max_tokens} "
        f"tokens, reply in {shape}.\n"
        f"# Save the reply exactly as the model gave it to replies/{name}.txt\n"
        f"# Paste everything below the line into the chat (into a system-instructions field, if the chat has one, "
        f"the part under SYSTEM).\n"
        "# ------------------------------------------------------------------------------------------------\n"
        f"SYSTEM:\n{system}\n\nUSER:\n{user}\n"
    )


class Exchange:
    name = "exchange"

    def __init__(self, root: Path, label: str = "chat"):
        self.root = Path(root)
        self.label = label
        self.replies = self.root / "replies"
        self.pending_dir = self.root / "pending"
        self.ledger_path = self.root / "ledger.json"
        self.replies.mkdir(parents=True, exist_ok=True)
        self.pending_dir.mkdir(parents=True, exist_ok=True)
        self.ledger: dict[str, str] = (
            json.loads(self.ledger_path.read_text(encoding="utf-8")) if self.ledger_path.exists() else {})
        changed = [name for name, digest in self.ledger.items() if self._read(name) is None or sha(self._read(name)) != digest]
        if changed:
            raise ReplyChanged(f"{len(changed)} replies changed or removed after a run used them ({', '.join(changed[:5])}): "
                               "that is tuning on the results; answer again in a new directory")
        for old in self.pending_dir.glob("*.txt"):  # rebuilt by this run
            old.unlink()
        self.pending: set[str] = set()
        self.used: set[str] = set()

    def _read(self, name: str) -> str | None:
        path = self.replies / f"{name}.txt"
        return path.read_text(encoding="utf-8").strip() if path.exists() else None

    async def complete(self, req: LlmRequest) -> LlmResponse:
        name = stem(req)
        text = self._read(name)
        if not text:
            if name not in self.pending:
                (self.pending_dir / f"{name}.txt").write_text(prompt_text(req, name), encoding="utf-8", newline="\n")
                self.pending.add(name)
            raise AwaitingReply(f"no reply yet for {name}: its prompt is in {self.pending_dir / (name + '.txt')}")
        digest = sha(text)
        if self.ledger.setdefault(name, digest) != digest:
            raise ReplyChanged(f"{name} changed during the run")
        if name not in self.used:
            self.used.add(name)
            self.ledger_path.write_text(json.dumps(self.ledger, indent=1, sort_keys=True) + "\n", encoding="utf-8")
        out_tokens = max(1, len(text) // 4)
        usage = Usage((len(req.system) + len(req.user)) // 4, out_tokens, estimated=True)
        finish = "length" if out_tokens > req.max_tokens else "stop"
        return LlmResponse(text, f"chat:{self.label}", usage, finish)

    async def stream(self, req: LlmRequest, on_delta: OnDelta) -> LlmResponse:
        resp = await self.complete(req)
        for piece in re.findall(r"\S+\s*", resp.text):
            await on_delta(piece)
        return resp


def status(root: Path) -> dict[str, Any]:
    pending = sorted(p.stem for p in (root / "pending").glob("*.txt"))
    answered = {p.stem for p in (root / "replies").glob("*.txt")}
    return {
        "pending": len(pending), "answered_pending": sorted(set(pending) & answered),
        "unanswered": sorted(set(pending) - answered), "replies": len(answered),
    }


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__.split("\n\n")[-1])
    s = status(Path(sys.argv[1]))
    by_kind: dict[str, int] = {}
    for name in s["unanswered"]:
        by_kind[name.split("-")[0]] = by_kind.get(name.split("-")[0], 0) + 1
    print(f"{s['replies']} replies; {len(s['unanswered'])} prompts waiting for one "
          f"({', '.join(f'{k} {v}' for k, v in sorted(by_kind.items())) or 'none'}); "
          f"{len(s['answered_pending'])} answered since the last run")
