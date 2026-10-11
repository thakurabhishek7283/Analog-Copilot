"""The reported analyzer, assembled with real 5 V power and bias, without bench sources."""

import json
import math

import circuit_core as cc
import pytest

from sim_runner.ngspice_batch import simulate


@pytest.mark.parametrize("frequency", [100, 1000, 5000])
def test_single_supply_three_channel_analyzer(reg, frequency):
    s = cc.Session(reg)

    def add(template, targets=None, ports=None):
        req = {"template": template, "targets": targets or {},
               "ports": {name: {"net": net} for name, net in (ports or {}).items()}}
        ins = cc.unwrap(s.insert_block(json.dumps(req)))
        cc.unwrap(s.apply_ops(json.dumps(ins["ops"]), "template"))
        block = json.loads(s.snapshot())["blocks"][ins["block"]]
        return {p["name"]: p["net"] for p in block["ports"]}

    source = add("sine_source", {"amplitude_v": "0.5", "freq_hz": str(frequency)})
    add("reg_7805")
    bias = add("bias_2v5")
    buffer = add("lm358_ac_buffer", ports={"in": source["out"], "vref": bias["out"]})
    low = add("rc_lowpass", {"fc_hz": "200"}, {"in": buffer["out"]})
    band = add("mfb_bandpass_5v", {"f0_hz": "1k"}, {"in": buffer["out"], "vref": bias["out"]})
    high = add("rc_highpass", {"fc_hz": "3k"}, {"in": buffer["out"]})

    assert not [e for e in cc.unwrap(s.erc("llm_block")) if e["severity"] == "error"]
    snapshot = json.loads(s.snapshot())
    assert "VEE" not in snapshot["nets"]
    assert snapshot["nets"]["V5"]["kind"] == {"kind": "power", "volts": 5.0}
    netlist = cc.unwrap(s.compile(json.dumps({"interactive": True, "transfer_checks": True})))
    result = simulate(netlist["text"], netlist["includes"], hash=netlist["hash"], timeout_s=10)
    assert result.status == "ok", result.log[-2000:]
    checks = cc.unwrap(cc.evaluate_checks(json.dumps(netlist["checks"]), json.dumps(result.meas)))
    assert len(checks) == 11
    assert all(c["pass"] for c in checks), [c for c in checks if not c["pass"]]
    # Check the actual transient responses too, not only the AC corner specifications.
    time = result.vec("time", "tran").data
    x = frequency / 1000
    expected = [(low, 0.5 / math.sqrt(1 + (frequency / 200) ** 2)),
                (band, 0.5 * (x / 2) / math.sqrt((1 - x * x) ** 2 + (x / 2) ** 2)),
                (high, 0.5 / math.sqrt(1 + (3000 / frequency) ** 2))]
    for ports, amplitude in expected:
        values = result.vec(f"v({ports['out'].lower()})", "tran").data
        settled = [v for t, v in zip(time, values, strict=True) if t >= time[-1] / 2]
        assert (max(settled) - min(settled)) / 2 == pytest.approx(amplitude, rel=0.06, abs=0.001)
