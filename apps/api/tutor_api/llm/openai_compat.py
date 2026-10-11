"""One HTTP adapter for OpenAI Responses and compatible chat completions endpoints: Gemini
(`generativelanguage.googleapis.com/v1beta/openai`) and DeepSeek on Azure AI Foundry
(`<resource>.services.ai.azure.com/openai/v1`). Plain httpx, no vendor SDK.

Structured output is either `json_schema` (the reply is constrained to the schema) or
`json_object` (any JSON object; the schema goes into the prompt), whichever the endpoint supports.
Each call has a 30 s deadline (LLD §6); timeouts, a dropped connection, 408, 429 and 5xx are
retryable, and the gateway does the retrying.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any, Literal

import httpx

from .base import LlmRequest, LlmResponse, OnDelta, ProviderError, Tier, Usage

JsonMode = Literal["json_schema", "json_object"]
CALL_TIMEOUT_S = 30.0


class OpenAICompatProvider:
    def __init__(
        self,
        name: str,
        base_url: str,
        api_key: str,
        models: dict[Tier, str],
        *,
        json_mode: JsonMode = "json_schema",
        api: Literal["chat", "responses"] = "chat",
        reasoning: bool = False,
        timeout_s: float = CALL_TIMEOUT_S,
        client: httpx.AsyncClient | None = None,
    ):
        self.name = name
        self.api = api
        self.url = base_url.rstrip("/") + ("/responses" if api == "responses" else "/chat/completions")
        self.models = models
        self.json_mode = json_mode
        # OpenAI's reasoning models (on Azure too) take `max_completion_tokens`, which also covers their
        # reasoning tokens, and only the default temperature.
        self.reasoning = reasoning
        self.timeout_s = timeout_s
        self.headers = {"Authorization": f"Bearer {api_key}"}
        # The deadline is enforced per call below; httpx's own timeouts (the same length) only bound a
        # stalled socket, and either one is a `timeout`. A request may set a longer deadline (a
        # reasoning model sends nothing while it thinks), which then bounds both.
        self.client = client or httpx.AsyncClient(timeout=httpx.Timeout(timeout_s, connect=10.0))

    def deadline(self, req: LlmRequest) -> float:
        return req.timeout_s or self.timeout_s

    async def aclose(self) -> None:
        await self.client.aclose()

    def body(self, req: LlmRequest, stream: bool) -> dict[str, Any]:
        if self.api == "responses":
            body: dict[str, Any] = {
                "model": self.models[req.tier], "instructions": req.system, "input": req.user,
                "max_output_tokens": req.max_tokens, "store": False,
            }
            if self.reasoning:
                if req.effort:
                    body["reasoning"] = {"effort": req.effort}
            else:
                body["temperature"] = req.temperature
            if req.schema is not None:
                body["text"] = {"format": {
                    "type": "json_schema", "name": req.schema_name, "schema": req.schema, "strict": False,
                }}
            if stream:
                body["stream"] = True
            return body
        user = req.user
        body: dict[str, Any] = {"model": self.models[req.tier]}
        if self.reasoning:
            body["max_completion_tokens"] = req.max_tokens
            if req.effort:
                body["reasoning_effort"] = req.effort
        else:
            body |= {"max_tokens": req.max_tokens, "temperature": req.temperature}
        if req.schema is not None:
            if self.json_mode == "json_schema":
                body["response_format"] = {
                    "type": "json_schema",
                    "json_schema": {"name": req.schema_name, "schema": req.schema},
                }
            else:
                body["response_format"] = {"type": "json_object"}
                user += "\n\nReply with one JSON object that matches this JSON Schema:\n" + json.dumps(req.schema)
        body["messages"] = [{"role": "system", "content": req.system}, {"role": "user", "content": user}]
        if stream:
            body["stream"] = True
            body["stream_options"] = {"include_usage": True}
        return body

    async def complete(self, req: LlmRequest) -> LlmResponse:
        t = self.deadline(req)
        try:
            async with asyncio.timeout(t):
                r = await self.client.post(self.url, json=self.body(req, False), headers=self.headers,
                                           timeout=httpx.Timeout(t, connect=10.0))
                check(r, r.text)
                data = r.json()
        except (TimeoutError, httpx.TimeoutException):
            raise ProviderError("timeout", f"{self.name}: no reply within {t:g} s", True) from None
        except (httpx.TransportError, OSError) as e:  # OSError: an SSL or socket error httpx did not wrap
            raise ProviderError("network", f"{self.name}: {type(e).__name__}: {e}", True) from None
        except ValueError as e:  # not JSON
            raise ProviderError("bad_response", f"{self.name}: {e}", True) from None
        if self.api == "responses":
            return read_response(data, self.models[req.tier])
        try:
            choice = data["choices"][0]
            text = choice["message"].get("content") or ""
        except (KeyError, IndexError, TypeError, AttributeError):
            raise ProviderError("bad_response", f"{self.name}: no choices in the reply", True) from None
        return LlmResponse(text, data.get("model") or self.models[req.tier], usage(data.get("usage")),
                           choice.get("finish_reason") or "stop")

    async def stream(self, req: LlmRequest, on_delta: OnDelta) -> LlmResponse:
        parts: list[str] = []
        used, model, finish = Usage(), self.models[req.tier], "stop"
        t = self.deadline(req)
        try:
            async with asyncio.timeout(t):
                async with self.client.stream("POST", self.url, json=self.body(req, True), headers=self.headers,
                                              timeout=httpx.Timeout(t, connect=10.0)) as r:
                    if r.status_code >= 400:
                        check(r, (await r.aread()).decode("utf-8", "replace"))
                    async for line in r.aiter_lines():
                        if not line.startswith("data:"):
                            continue
                        payload = line[5:].strip()
                        if payload == "[DONE]":
                            break
                        chunk = json.loads(payload)
                        if self.api == "responses":
                            kind = chunk.get("type")
                            if kind == "response.output_text.delta":
                                await on_delta(chunk["delta"])
                            elif kind in ("response.completed", "response.incomplete", "response.failed"):
                                return read_response(chunk.get("response"), model)
                            elif kind == "error":
                                code = chunk.get("code") or "bad_response"
                                raise ProviderError(code, chunk.get("message") or "Response stream failed",
                                                    code in ("server_error", "rate_limit_exceeded"))
                            continue
                        model = chunk.get("model") or model
                        if chunk.get("usage"):
                            used = usage(chunk["usage"])
                        for choice in chunk.get("choices") or []:
                            if text := (choice.get("delta") or {}).get("content"):
                                parts.append(text)
                                await on_delta(text)
                            finish = choice.get("finish_reason") or finish
        except (TimeoutError, httpx.TimeoutException):
            raise ProviderError("timeout", f"{self.name}: stream not finished within {t:g} s", True) from None
        except (httpx.TransportError, OSError) as e:
            # OSError: httpcore does not wrap an SSL error raised while reading the body (live, 2026-05-22:
            # "ssl.SSLError: record layer failure" mid-answer reached the router as an internal error).
            raise ProviderError("network", f"{self.name}: {type(e).__name__}: {e}", True) from None
        except (ValueError, KeyError, TypeError, AttributeError) as e:
            raise ProviderError("bad_response", f"{self.name}: {e}", True) from None
        if self.api == "responses":
            raise ProviderError("bad_response", "Response stream ended without a terminal event", True)
        return LlmResponse("".join(parts), model, used, finish)


def read_response(data: Any, model: str) -> LlmResponse:
    """Read message text, skipping reasoning items, and preserve token-limit failures."""
    if not isinstance(data, dict) or data.get("status") not in ("completed", "incomplete", "failed"):
        raise ProviderError("bad_response", "Missing response status", True)
    if data["status"] == "failed":
        raise ProviderError("bad_response", (data.get("error") or {}).get("message") or "Response failed", True)
    finish = "stop"
    if data["status"] == "incomplete":
        reason = (data.get("incomplete_details") or {}).get("reason")
        if reason != "max_output_tokens":
            raise ProviderError("bad_response", f"Response incomplete: {reason}", False)
        finish = "length"
    try:
        text = []
        for item in data["output"]:
            if item.get("type") != "message":
                continue
            for content in item["content"]:
                if content.get("type") == "refusal":
                    raise ProviderError("refusal", content.get("refusal") or "Model refused the request", False)
                if content.get("type") == "output_text":
                    text.append(content["text"])
        if not any(text) and finish != "length":
            raise ProviderError("bad_response", "Response completed without message text", True)
        return LlmResponse("".join(text), data.get("model") or model, usage(data.get("usage")), finish)
    except (KeyError, TypeError, AttributeError, ValueError):
        raise ProviderError("bad_response", "Malformed response output", True) from None


def check(r: httpx.Response, body: str) -> None:
    if r.status_code < 400:
        return
    details: list[Any] = []
    try:
        detail = json.loads(body)
        detail = detail[0] if isinstance(detail, list) else detail  # Gemini sometimes wraps it
        message = str((detail.get("error") or {}).get("message") or body)
        details = [d for d in (detail.get("error") or {}).get("details") or [] if isinstance(d, dict)]
    except (ValueError, AttributeError, IndexError):
        message = body
    # Gemini says which quota ran out and when to retry in the body (QuotaFailure, RetryInfo), not in headers.
    quota = next((v.get("quotaId") for d in details for v in d.get("violations") or [] if v.get("quotaId")), None)
    delay = next((seconds(d["retryDelay"]) for d in details if d.get("retryDelay")), None)
    message = f"HTTP {r.status_code}: " + (f"[{quota}] " if quota else "") + message[:300]
    if r.status_code == 429:
        if quota and "PerDay" in quota:  # retrying today cannot help: let the fallback provider answer
            raise ProviderError("quota_exhausted", message, False)
        raise ProviderError("rate_limited", message, True, retry_after(r) or delay)
    if r.status_code == 408 or r.status_code >= 500:
        raise ProviderError("server_error", message, True, retry_after(r))
    if r.status_code in (401, 403):
        raise ProviderError("auth", message, False)
    raise ProviderError("bad_request", message, False)


def retry_after(r: httpx.Response) -> float | None:
    try:
        return float(r.headers["retry-after"])
    except (KeyError, ValueError):
        return None


def seconds(text: Any) -> float | None:
    """A protobuf Duration as JSON ("48s", "48.220090438s")."""
    try:
        return float(str(text).removesuffix("s"))
    except ValueError:
        return None


def usage(u: dict[str, Any] | None) -> Usage:
    if not u:
        return Usage()
    cached = (u.get("input_tokens_details") or u.get("prompt_tokens_details") or {}).get("cached_tokens") or 0
    return Usage(int(u.get("input_tokens") or u.get("prompt_tokens") or 0),
                 int(u.get("output_tokens") or u.get("completion_tokens") or 0), int(cached))
