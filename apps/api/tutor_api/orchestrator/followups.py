"""Requested measurements and physical-fit evidence, without changing the committed circuit."""

from __future__ import annotations

import json
import math
from hashlib import sha256

import circuit_core as cc
from sim_runner import client as sim_client
from sim_runner.protocol import SimRefused, SimRequest

from .. import breadboard
from .verifier import failed_checks


async def run(job, plan, session: cc.Session) -> tuple[dict, list[str]]:
    evidence, problems = {}, []
    snapshot = json.loads(session.snapshot())
    test = getattr(plan, "transient_tests", None)
    if test:
        source, _ = test.source
        sources = [ref for ref, part in snapshot["parts"].items()
                   if part.get("block") == source and part["part"] == "vsource_sine"]
        if len(sources) != 1:
            return {}, [f"Transient tests need exactly one sine source in {source}."]
        runs = []
        for frequency in test.frequencies:
            fork = session.fork()
            cc.unwrap(fork.apply_ops(json.dumps([{"op": "part.set_param", "body": {
                "refdes": sources[0], "key": "frequency", "value": str(frequency)}}]), "user"))
            # Allow low-frequency filter transients to settle, while keeping the step fine enough
            # for the selected sine. These analyses exist only on the scratch copy.
            stop, step = max(16 / frequency, 0.02), 1 / (200 * frequency)
            stop = min(stop, step * 100_000)
            analyses = [a for a in snapshot["analyses"] if a["type"] != "tran"]
            analyses.append({"type": "tran", "t_step": step, "t_stop": stop})
            netlist = cc.unwrap(fork.compile(json.dumps({"interactive": True, "transfer_checks": True,
                                                        "analyses": analyses})))
            text = netlist["text"]
            outputs = []
            cards = []
            for index, (bid, port) in enumerate(test.outputs):
                block = snapshot["blocks"][bid]
                net = next(p["net"] for p in block["ports"] if p["name"] == port)
                outputs.append({"block": bid, "port": port, "title": block["title"], "net": net})
                for name, measure in (("max", "max"), ("min", "min"), ("bias", "avg")):
                    cards.append(f".meas tran test_{index}_{name} {measure} v({net.lower()}) from={stop/2} to={stop}")
            text = text.replace("\n.end\n", "\n" + "\n".join(cards) + "\n.end\n")
            # The worker checks the hash of the exact deck, including these extra measurements.
            request = SimRequest(text, netlist["includes"], sha256(text.encode()).hexdigest(), job.ctx.registry_version)
            try:
                result = await sim_client.simulate(job.ctx.redis, request, wait_s=10)
            except (TimeoutError, SimRefused) as exc:
                problems.append(f"Transient at {frequency:g} Hz could not run: {exc}")
                continue
            if result.status != "ok":
                problems.append(f"Transient at {frequency:g} Hz failed: {result.status}")
                continue
            checks = netlist["checks"]
            for check in checks:
                if check["block"] == source and check["name"] == "freq_hz":
                    check["target"] = frequency  # expected stimulus for this test, not a circuit edit
            checked = cc.unwrap(cc.evaluate_checks(json.dumps(checks), json.dumps(result.meas)))
            failed = failed_checks(checked)
            if failed:
                problems += [f"Transient at {frequency:g} Hz: {p['message']}" for p in failed]
                continue
            for index, output in enumerate(outputs):
                values = [result.meas.get(f"test_{index}_{name}") for name in ("max", "min", "bias")]
                if any(v is None or not math.isfinite(v) for v in values):
                    problems.append(f"Transient at {frequency:g} Hz: no complete measurements for {output['block']}.{output['port']}")
                    break
                output.update(amplitude_v=(values[0] - values[1]) / 2, bias_v=values[2])
            else:
                runs.append({"frequency_hz": frequency, "source": source, "outputs": outputs})
        evidence["transient_tests"] = runs
    limit = getattr(plan, "max_breadboards", None)
    if limit:
        try:
            layout = breadboard.fit(snapshot, job.prompts.bundle, limit)
            evidence["breadboard_fit"] = {"valid": True, "boards": layout.boards, "max_boards": limit,
                                          "parts": len(layout.parts), "jumpers": len(layout.jumpers), "rev": session.rev}
        except ValueError as exc:
            problems.append(f"Breadboard fit: {exc}")
    return evidence, problems


def explanation(evidence: dict) -> str:
    lines = []
    for run in evidence.get("transient_tests", []):
        outputs = run["outputs"]
        levels = "; ".join(f"{o['block']}.{o['port']} ({o['title']}): {o['amplitude_v']:.3g} V peak, "
                           f"{o['bias_v']:.3g} V DC" for o in outputs)
        strongest = max(outputs, key=lambda o: o["amplitude_v"])
        lines.append(f"At {run['frequency_hz']:g} Hz: {levels}. The {strongest['title']} channel has the largest signal.")
    if evidence.get("breadboard_fit"):
        lines.append(f"The supported packages and validated connections fit on {evidence['breadboard_fit']['boards']} breadboards.")
    return "\n\n".join(lines)
