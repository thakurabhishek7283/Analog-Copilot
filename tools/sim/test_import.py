"""SPICE import (LLD §17 D3): a registry model file brought in through the allow-list parser and
re-emitted from its tree simulates exactly like the same file loaded with `.include`. Each test
puts the registry part and the imported component in two identical benches of one circuit, so a
single simulation compares them."""

from __future__ import annotations

import circuit_core as cc
import pytest

from circuits import Bench, tran
from sim_runner.ngspice_batch import REPO

# Both copies solve in one deck; they differ only by matrix ordering, inside reltol.
REL = 1e-3


def imported(name: str, file: str, subckt: str, pins: list[tuple[str, str, str]]) -> dict:
    """A library component importing `registry/models/<file>`; pins are (name, type, port)."""
    return {
        "part": {
            "id": f"lib:{name}",
            "category": "X",
            "prefix": "U",
            "title": f"{subckt} (imported)",
            "symbol": "auto",
            "pins": [{"name": n, "num": i + 1, "type": t} for i, (n, t, _) in enumerate(pins)],
        },
        "version": 1,
        "model": {
            "kind": "spice",
            "text": (REPO / "registry" / "models" / file).read_text(encoding="utf-8"),
            "subckt": subckt,
            "pins": {n: port for n, _, port in pins},
        },
        "provenance": {"kind": "import", "source": f"registry/models/{file}", "licence": "PolyForm-Noncommercial-1.0.0"},
    }


OPAMP_PINS = [("INP", "input", "inp"), ("INM", "input", "inm"), ("VCC", "power_pos", "vcc"), ("VEE", "power_neg", "vee"), ("OUT", "output", "out")]


def same(r, a: str, b: str) -> None:
    va, vb = r.vec(a, "tran").data, r.vec(b, "tran").data
    scale = max(abs(x) for x in va)
    assert max(abs(x - y) for x, y in zip(va, vb, strict=True)) <= REL * scale, (a, b)


def gain_of_two(b: Bench, k: int, opamp: str, pins: tuple[str, str, str, str, str], ref: str) -> None:
    inp, inm, vcc, vee, out = pins
    b.part(f"R{2 * k + 1}", "resistor_th", resistance="10k").part(f"R{2 * k + 2}", "resistor_th", resistance="10k")
    b.net("IN", f"{opamp}.{inp}").net(f"NM{k}", f"{opamp}.{inm}", f"R{2 * k + 1}.2", f"R{2 * k + 2}.1")
    b.net(f"OUT{k}", f"{opamp}.{out}", f"R{2 * k + 1}.1").net(ref, f"R{2 * k + 2}.2")
    b.net("VCC", f"{opamp}.{vcc}").net("VEE" if ref == "GND" else "GND", f"{opamp}.{vee}")


@pytest.mark.parametrize("model", ["lm358", "tl072"])
def test_opamp_import_simulates_like_the_include(reg, model):
    b = Bench(reg).op("lib.add", {"def": imported(f"{model}_imp", f"{model}.lib", model.upper(), OPAMP_PINS)})
    if model == "lm358":  # one 5 V supply, around 2.5 V
        b.part("V1", "vsource_dc", voltage="5").part("V2", "vsource_dc", voltage="2.5")
        b.part("V3", "vsource_sine", offset="2.5", amplitude="0.4", frequency="1k")
        b.net("VCC", "V1.P").net("REF", "V2.P").net("GND", "V1.N", "V2.N", "V3.N")
        ref = "REF"
    else:  # ±12 V
        b.part("V1", "vsource_dc", voltage="12").part("V2", "vsource_dc", voltage="12")
        b.part("V3", "vsource_sine", amplitude="4", frequency="1k")
        b.net("VCC", "V1.P").net("GND", "V1.N", "V2.P", "V3.N").net("VEE", "V2.N")
        ref = "GND"
    b.net("IN", "V3.P")
    b.part("U1", f"opamp_{model}").part("U2", f"lib:{model}_imp")
    gain_of_two(b, 0, "U1", ("INP_A", "INM_A", "VCC", "VEE", "OUT_A"), ref)
    gain_of_two(b, 1, "U2", ("INP", "INM", "VCC", "VEE", "OUT"), ref)
    r = b.analyses(tran(2e-6, 3e-3)).simulate()
    assert r.status == "ok", r.log[-2000:]
    same(r, "v(out0)", "v(out1)")


def test_regulator_import_simulates_like_the_include(reg):
    pins = [("IN", "power_pos", "in"), ("GND", "power_neg", "gnd"), ("OUT", "output", "out")]
    b = Bench(reg).op("lib.add", {"def": imported("lm7805_imp", "lm7805.lib", "LM7805", pins)})
    b.part("V1", "vsource_sine", offset="9", amplitude="3", frequency="100")
    b.part("U1", "reg_lm7805").part("U2", "lib:lm7805_imp")
    b.part("R1", "resistor_th", resistance="100").part("R2", "resistor_th", resistance="100")
    b.net("VIN", "V1.P", "U1.IN", "U2.IN").net("GND", "V1.N", "U1.GND", "U2.GND", "R1.2", "R2.2")
    b.net("OUT0", "U1.OUT", "R1.1").net("OUT1", "U2.OUT", "R2.1")
    r = b.analyses(tran(20e-6, 30e-3)).simulate()
    assert r.status == "ok", r.log[-2000:]
    same(r, "v(out0)", "v(out1)")


def test_timer_import_simulates_like_the_include(reg):
    names = ["GND", "TRIG", "OUT", "RESET", "CTRL", "THR", "DIS", "VCC"]
    kinds = ["power_neg", "input", "output", "input", "passive", "input", "passive", "power_pos"]
    pins = [(n, t, n.lower()) for n, t in zip(names, kinds, strict=True)]
    b = Bench(reg).op("lib.add", {"def": imported("ne555_imp", "ne555.lib", "NE555", pins)})
    b.part("V1", "vsource_dc", voltage="9").net("VCC", "V1.P").net("GND", "V1.N")
    for k, u in enumerate(["U1", "U2"]):
        b.part(u, "timer_ne555" if k == 0 else "lib:ne555_imp")
        r1, r2, r3, c1, c2 = (f"R{3 * k + 1}", f"R{3 * k + 2}", f"R{3 * k + 3}", f"C{2 * k + 1}", f"C{2 * k + 2}")
        b.part(r1, "resistor_th", resistance="1k").part(r2, "resistor_th", resistance="10k")
        b.part(r3, "resistor_th", resistance="10k")
        b.part(c1, "cap_film", capacitance="100n").part(c2, "cap_film", capacitance="10n")
        b.net("VCC", f"{u}.VCC", f"{r1}.1", f"{u}.RESET").net(f"DIS{k}", f"{r1}.2", f"{r2}.1", f"{u}.DIS")
        b.net(f"TH{k}", f"{r2}.2", f"{u}.THR", f"{u}.TRIG", f"{c1}.1").net(f"CTRL{k}", f"{u}.CTRL", f"{c2}.1")
        b.net(f"OUT{k}", f"{u}.OUT", f"{r3}.1").net("GND", f"{u}.GND", f"{c1}.2", f"{c2}.2", f"{r3}.2")
    r = b.analyses(tran(1e-6, 6e-3)).simulate()
    assert r.status == "ok", r.log[-2000:]
    same(r, "v(th0)", "v(th1)")


def test_netlist_holds_no_user_text(reg):
    """What reaches the simulator is generated: no comment, no vendor name, no original line."""
    d = imported("lm358_imp", "lm358.lib", "LM358", OPAMP_PINS)
    b = Bench(reg).op("lib.add", {"def": d}).part("U1", "lib:lm358_imp")
    text = b.netlist()["text"]
    for line in d["model"]["text"].splitlines():
        if line.strip() and not line.startswith("*"):
            assert line.strip() not in text, line
    assert "LM358" not in text and "dclamp" not in text
