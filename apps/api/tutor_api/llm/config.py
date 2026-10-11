"""The gateway from the environment.

- `LLM_PROVIDER`: unset (generation is unavailable), `fake` (replies from `LLM_FAKE_SCRIPT`, a JSON
  file; see `fake.py`), `replay` (a cassette at `LLM_CASSETTE`), or a real provider: `gemini`,
  `deepseek`, `openai`.
- `LLM_FALLBACK_PROVIDER`: optional, another real provider tried after the first gives up.
- A real provider `P` takes its key from `P_API_KEY`, its endpoint from `P_BASE_URL` (Gemini and
  OpenAI have defaults), and its models from `P_MODEL_LARGE` / `P_MODEL_SMALL`, else `P_MODEL`
  for both, else, for the primary provider only, `LLM_MODEL_LARGE` / `LLM_MODEL_SMALL`.
  `P_REASONING=1` for a reasoning model (GPT-5 and later, also on Azure): `max_completion_tokens`
  instead of `max_tokens`, and no temperature (only the default is accepted).
- `LLM_TIER_ASK`, `LLM_TIER_WHAT_CHANGED`: the tier the tutor answers a question, or explains an
  edit, with: `small` (default, LLD §6, §9) or `large`.
- With `OPENAI_SOL_API_KEY`, OpenAI's large tier uses Sol via Responses and its small tier uses
  `OPENAI_LUNA_API_KEY` (or legacy `OPENAI_API_KEY`) via Chat Completions. Each has an optional
  `OPENAI_SOL_*` / `OPENAI_LUNA_*` `BASE_URL` and `MODEL`; endpoints fall back to `OPENAI_BASE_URL`.
"""

from __future__ import annotations

import os
from collections.abc import Mapping

from .base import LlmRequest, LlmResponse, OnDelta, Provider, Tier
from .cassette import Replayer
from .fake import FakeProvider
from .gateway import Gateway
from .openai_compat import OpenAICompatProvider

# name -> (default base URL, structured output mode)
REAL = {
    "gemini": ("https://generativelanguage.googleapis.com/v1beta/openai", "json_schema"),
    # Azure AI Foundry's OpenAI v1 endpoint; DeepSeek models take json_object, not json_schema.
    "deepseek": (None, "json_object"),
    "openai": ("https://api.openai.com/v1", "json_schema"),
}


# The tutor's request kinds and their default tiers (LLD §6: Ask uses the small model; §9: so does
# "What changed?").
TUTOR_TIERS: dict[str, Tier] = {"ask": "small", "what_changed": "small"}


class OpenAITiers:
    """Sol and Luna have separate Azure credentials and endpoints, behind the existing gateway."""

    name = "openai"

    def __init__(self, large: OpenAICompatProvider, small: OpenAICompatProvider):
        self.tiers = {"large": large, "small": small}
        self.models = {tier: provider.models[tier] for tier, provider in self.tiers.items()}

    async def complete(self, req: LlmRequest) -> LlmResponse:
        return await self.tiers[req.tier].complete(req)

    async def stream(self, req: LlmRequest, on_delta: OnDelta) -> LlmResponse:
        return await self.tiers[req.tier].stream(req, on_delta)

    async def aclose(self) -> None:
        for provider in self.tiers.values():
            await provider.aclose()


def tutor_tiers_from_env(env: Mapping[str, str] | None = None) -> dict[str, Tier]:
    env = os.environ if env is None else env
    tiers = dict(TUTOR_TIERS)
    for kind in tiers:
        name = f"LLM_TIER_{kind.upper()}"
        value = (env.get(name) or "").strip().lower()
        if value:
            if value not in ("small", "large"):
                raise RuntimeError(f"{name} must be small or large, not {value!r}")
            tiers[kind] = value  # type: ignore[assignment]
    return tiers


def provider_from_env(name: str, env: Mapping[str, str], *, primary: bool) -> Provider:
    if name not in REAL:
        raise RuntimeError(f"unknown LLM provider {name!r} (one of {', '.join(REAL)})")
    default_url, json_mode = REAL[name]
    p = name.upper()

    def get(key: str) -> str:
        return (env.get(key) or "").strip()

    if name == "openai" and get("OPENAI_SOL_API_KEY"):
        sol_key = get("OPENAI_SOL_API_KEY")
        luna_key = get("OPENAI_LUNA_API_KEY") or get("OPENAI_API_KEY")
        for variable, value in (("OPENAI_SOL_API_KEY", sol_key), ("OPENAI_LUNA_API_KEY", luna_key)):
            if not value or "REPLACE_ME" in value:
                raise RuntimeError(f"{variable} is not set")
        sol_model = get("OPENAI_SOL_MODEL") or "gpt-6.1-sol"
        luna_model = get("OPENAI_LUNA_MODEL") or get("OPENAI_MODEL") or "gpt-6-luna"
        sol = OpenAICompatProvider(
            name, get("OPENAI_SOL_BASE_URL") or get("OPENAI_BASE_URL") or default_url, sol_key,
            {"large": sol_model, "small": sol_model}, api="responses", reasoning=True,
        )
        luna = OpenAICompatProvider(
            name, get("OPENAI_LUNA_BASE_URL") or get("OPENAI_BASE_URL") or default_url, luna_key,
            {"large": luna_model, "small": luna_model}, reasoning=True,
        )
        return OpenAITiers(sol, luna)
    key = (get("OPENAI_LUNA_API_KEY") if name == "openai" else "") or get(f"{p}_API_KEY")
    if not key or "REPLACE_ME" in key:
        raise RuntimeError(f"{p}_API_KEY is not set")
    base_url = get(f"{p}_BASE_URL") or default_url
    if not base_url:
        raise RuntimeError(f"{p}_BASE_URL is not set")
    models = {}
    for tier in ("large", "small"):
        model = get(f"{p}_MODEL_{tier.upper()}") or get(f"{p}_MODEL") or (get(f"LLM_MODEL_{tier.upper()}") if primary else "")
        if not model:
            raise RuntimeError(f"no {tier} model for {name}: set {p}_MODEL_{tier.upper()} or {p}_MODEL")
        models[tier] = model
    reasoning = get(f"{p}_REASONING").lower() in ("1", "true", "yes")
    return OpenAICompatProvider(name, base_url, key, models, json_mode=json_mode, reasoning=reasoning)


def gateway_from_env(env: Mapping[str, str] | None = None) -> Gateway | None:
    env = os.environ if env is None else env
    name = (env.get("LLM_PROVIDER") or "").strip().lower()
    if not name:
        return None
    if name == "fake":
        script = env.get("LLM_FAKE_SCRIPT")
        if not script:
            raise RuntimeError("LLM_PROVIDER=fake needs LLM_FAKE_SCRIPT (a JSON file of scripted replies)")
        return Gateway(FakeProvider.from_file(script))
    if name == "replay":
        cassette = env.get("LLM_CASSETTE")
        if not cassette:
            raise RuntimeError("LLM_PROVIDER=replay needs LLM_CASSETTE")
        return Gateway(Replayer.from_file(cassette))
    primary = provider_from_env(name, env, primary=True)
    fallback_name = (env.get("LLM_FALLBACK_PROVIDER") or "").strip().lower()
    fallback = provider_from_env(fallback_name, env, primary=False) if fallback_name else None
    return Gateway(primary, fallback)
