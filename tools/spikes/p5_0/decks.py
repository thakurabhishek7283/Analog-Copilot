"""P5.0 spike decks. Writes requests.json: [{name, netlist, includes, hash}].

Spike 1 (hierarchy): a reduced three-band analyser on the LM358 model and the 7805, emitted three
ways from one hierarchical description:
  flat   - every element at top level, names and nodes mangled by instance path (today's compiler)
  mono   - one .subckt per (definition, values): values written into the body, no numparam
  param  - one .subckt per definition with `params:` and {expr} values (ngspice numparam)
Spike 2 (behavioral laws): B/G-source models with hard and smooth limits, and a 2nd-order transfer
function realised as a state space.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field

# ------------------------------------------------------------------ a tiny hierarchical model


@dataclass
class Def:
    name: str
    ports: list[str]
    elems: list[tuple]  # (name, nodes, tail) | ("X", inst, nodes, defname, values)
    params: dict[str, str] = field(default_factory=dict)  # name -> default (param variant)


def sk_lp(r: str, c1: str, c2: str) -> list[tuple]:
    return [
        ("R1", ["in", "n_a"], r), ("R2", ["n_a", "n_b"], r),
        ("C1", ["n_a", "out"], c1), ("C2", ["n_b", "ref"], c2),
        ("XU1_A", ["n_b", "out", "vcc", "vee", "out"], "LM358"),
    ]


def sk_hp(c: str, r1: str, r2: str) -> list[tuple]:
    return [
        ("C1", ["in", "n_a"], c), ("C2", ["n_a", "n_b"], c),
        ("R1", ["n_a", "out"], r1), ("R2", ["n_b", "ref"], r2),
        ("XU1_A", ["n_b", "out", "vcc", "vee", "out"], "LM358"),
    ]


def mfb_bp(r1: str, r2: str, r3: str, c: str) -> list[tuple]:
    return [
        ("R1", ["in", "n_a"], r1), ("R2", ["n_a", "ref"], r2),
        ("C1", ["n_a", "n_m"], c), ("C2", ["n_a", "out"], c), ("R3", ["n_m", "out"], r3),
        ("XU1_A", ["ref", "n_m", "vcc", "vee", "out"], "LM358"),
    ]


def gain(rf: str, rg: str) -> list[tuple]:
    # Non-inverting, gain leg returned to ref (not GND): the bias is not amplified.
    return [
        ("RF", ["out", "n_m"], rf), ("RG", ["n_m", "ref"], rg),
        ("XU1_A", ["in", "n_m", "vcc", "vee", "out"], "LM358"),
    ]


FILTER_PORTS = ["in", "out", "ref", "vcc", "vee"]

# Values per channel: filter values, then gain values.
CHANNELS = {
    "a": ("sk_lp", {"r": "10k", "c1": "120n", "c2": "56n"}, {"rf": "5.1k", "rg": "10k"}),
    "b": ("mfb_bp", {"r1": "33k", "r2": "4.7k", "r3": "62k", "c": "10n"}, {"rf": "2k", "rg": "10k"}),
    "c": ("sk_hp", {"c": "10n", "r1": "3.6k", "r2": "7.5k"}, {"rf": "8.2k", "rg": "10k"}),
}
BUILDERS = {"sk_lp": sk_lp, "sk_hp": sk_hp, "mfb_bp": mfb_bp, "gain": gain}


def top_elems() -> list[tuple]:
    e: list[tuple] = [
        ("VIN", ["vin", "0"], "DC 9"),
        ("XREG", ["vin", "0", "v5"], "LM7805"),
        ("CREG1", ["vin", "0"], "330n"), ("CREG2", ["v5", "0"], "100n"),
        # 2.5 V reference: divider, follower, bulk capacitor
        ("RD1", ["v5", "vref_div"], "10k"), ("RD2", ["vref_div", "0"], "10k"),
        ("XUR_A", ["vref_div", "vref", "v5", "0", "vref"], "LM358"),
        ("CREF", ["vref", "0"], "10u"),
        # input: 0.5 V peak sine on the 2.5 V bias, buffered
        ("VSIG", ["src", "0"], "DC 2.5 AC 0.5 SIN(2.5 0.5 1k)"),
        ("XUB_A", ["src", "buf", "v5", "0", "buf"], "LM358"),
    ]
    for ch in CHANNELS:
        e.append(("X", f"ch{ch}", ["buf", f"out_{ch}", "vref", "v5", "0"], f"ch_{ch}", {}))
    return e


def channel_def(ch: str) -> Def:
    kind, fvals, gvals = CHANNELS[ch]
    return Def(
        f"ch_{ch}", ["in", "out", "ref", "vcc", "vee"],
        [("X", "f", ["in", "filt", "ref", "vcc", "vee"], kind, fvals),
         ("X", "g", ["filt", "out", "ref", "vcc", "vee"], "gain", gvals)],
    )


# ------------------------------------------------------------------ emitters


def leaf_line(name: str, nodes: list[str], tail: str) -> str:
    return f"{name} {' '.join(nodes)} {tail}"


def emit_flat() -> list[str]:
    lines: list[str] = []

    def walk(elems, path: str, nodemap):
        for el in elems:
            if el[0] == "X":
                _, inst, nodes, defname, values = el
                sub_path = f"{path}{inst}_"
                if defname.startswith("ch_"):
                    d = channel_def(defname[3:])
                    body, ports = d.elems, d.ports
                else:
                    body, ports = BUILDERS[defname](**values), FILTER_PORTS
                outer = [nodemap(n) for n in nodes]
                inner = dict(zip(ports, outer))
                walk(body, sub_path, lambda n, inner=inner, p=sub_path: inner.get(n) or ("0" if n == "0" else f"{p}{n}"))
            else:
                name, nodes, tail = el
                flat_name = name[0] + path + name[1:] if path else name
                lines.append(leaf_line(flat_name, [nodemap(n) for n in nodes], tail))

    walk(top_elems(), "", lambda n: n)
    return lines


def emit_mono() -> list[str]:
    subckts: dict[str, list[str]] = {}

    def spec(defname: str, values: dict) -> str:
        if defname.startswith("ch_"):
            d = channel_def(defname[3:])
            body = []
            for el in d.elems:
                _, inst, nodes, sub, vals = el
                body.append(f"X{inst} {' '.join(nodes)} {spec(sub, vals)}")
            name = defname
            ports = d.ports
        else:
            # One subckt per (definition, values): the name carries a short value key.
            key = "_".join(v.replace(".", "p") for v in values.values())
            name = f"{defname}__{key}"
            body = [leaf_line(*el) for el in BUILDERS[defname](**values)]
            ports = FILTER_PORTS
        if name not in subckts:
            subckts[name] = [f".subckt {name} {' '.join(ports)}", *body, f".ends {name}"]
        return name

    top = []
    for el in top_elems():
        if el[0] == "X":
            _, inst, nodes, defname, values = el
            top.append(f"X{inst} {' '.join(nodes)} {spec(defname, values)}")
        else:
            top.append(leaf_line(*el))
    return [l for name in sorted(subckts) for l in subckts[name]] + top


def emit_param() -> list[str]:
    defaults = {
        "sk_lp": {"r": "10k", "c1": "10n", "c2": "10n"},
        "sk_hp": {"c": "10n", "r1": "10k", "r2": "10k"},
        "mfb_bp": {"r1": "10k", "r2": "10k", "r3": "10k", "c": "10n"},
        "gain": {"rf": "10k", "rg": "10k"},
    }
    lines: list[str] = []
    for defname, d in defaults.items():
        params = " ".join(f"{k}={v}" for k, v in d.items())
        body = [leaf_line(*el) for el in BUILDERS[defname](**{k: "{" + k + "}" for k in d})]
        lines += [f".subckt {defname} {' '.join(FILTER_PORTS)} params: {params}", *body, f".ends {defname}"]
    for ch in CHANNELS:
        d = channel_def(ch)
        lines.append(f".subckt {d.name} {' '.join(d.ports)}")
        for _, inst, nodes, sub, vals in d.elems:
            pv = " ".join(f"{k}={v}" for k, v in vals.items())
            lines.append(f"X{inst} {' '.join(nodes)} {sub} {pv}")
        lines.append(f".ends {d.name}")
    for el in top_elems():
        if el[0] == "X":
            _, inst, nodes, defname, _ = el
            lines.append(f"X{inst} {' '.join(nodes)} {defname}")
        else:
            lines.append(leaf_line(*el))
    return lines


def analyser_deck(variant: str) -> str:
    body = {"flat": emit_flat, "mono": emit_mono, "param": emit_param}[variant]()
    internal = "cha_f_n_a" if variant == "flat" else "xcha.xf.n_a"
    meas = []
    for ch in CHANNELS:
        for f in (100, 1000, 5000):
            meas.append(f".meas ac {ch}_g{f} find vm(out_{ch}) at={f}")
        meas.append(f".meas tran {ch}_max max v(out_{ch}) from=10m to=20m")
        meas.append(f".meas tran {ch}_min min v(out_{ch}) from=10m to=20m")
    meas += [
        f".meas ac lp_na_1k find vm({internal}) at=1000",
        ".meas tran vref_avg avg v(vref) from=10m to=20m",
        ".meas tran v5_avg avg v(v5) from=10m to=20m",
    ]
    return "\n".join([
        "* p50 analyser " + variant,
        ".include models/lm358.lib",
        ".include models/lm7805.lib",
        *body,
        ".options reltol=1e-3 gmin=1e-12",
        ".save all",
        ".op",
        ".ac dec 50 10 20k",
        ".tran 10u 20m",
        *meas,
        ".end",
        "",
    ])


# ------------------------------------------------------------------ spike 2: behavioral laws

W = 0.005  # smooth-limit corner width, volts


def smax(a: str, b: str, w: float = W) -> str:
    # max(a,b) + w*ln(1+exp(-|a-b|/w)): continuous slope, no overflow
    return f"(max({a},{b})+{w}*ln(1+exp(-abs(({a})-({b}))/{w})))"


def smin(a: str, b: str, w: float = W) -> str:
    return f"(min({a},{b})-{w}*ln(1+exp(-abs(({a})-({b}))/{w})))"


def limit(x: str, lo: str, hi: str, smooth: bool) -> str:
    return smin(smax(x, lo), hi) if smooth else f"min(max({x},{lo}),{hi})"


def bopamp(smooth: bool, windup: bool = False) -> list[str]:
    """Behavioral LM358-like op-amp from laws: Rin, Aol=1e5 with a 10 Hz pole (G into C),
    output limited to (vee+0.02, vcc-1.5), Rout 50, 0.7 mA supply current. Without `windup`, the
    state n1 is bounded too: a 1 S conductance pulls it back whenever it leaves the limits."""
    clip = limit("v(n1)", "v(vee)+0.02", "v(vcc)-1.5", smooth)
    name = f"BOPAMP_{'S' if smooth else 'H'}{'W' if windup else ''}"
    bound = [] if windup else [f"bbound n1 0 i=v(n1)-{clip}"]
    return [
        f".subckt {name} inp inm vcc vee out",
        "rin inp inm 1e12",
        "g1 0 n1 inp inm 10u",
        "r1 n1 0 10g",
        "c1 n1 0 1.592p",
        *bound,
        f"b1 n2 0 v={clip}",
        "rout n2 out 50",
        "rq vcc vee 7.1k",
        ".ends",
    ]


def schmitt_deck(smooth: bool, windup: bool = False, model: str = "") -> str:
    """Single-supply Schmitt trigger: thresholds 1.667 V / 3.333 V nominal for a rail-to-rail
    output; with the 3.5 V upper clamp they are computed in compare.py."""
    s = model or ("S" if smooth else "H") + ("W" if windup else "")
    sub = [".include models/lm358.lib"] if model else bopamp(smooth, windup)
    sub_name = model or f"BOPAMP_{s}"
    return "\n".join([
        f"* p50 schmitt {s}",
        *sub,
        "VCC vcc 0 DC 5",
        "VIN in 0 DC 2.5 SIN(2.5 2 100)",
        "R1 vcc ninp 10k", "R2 ninp 0 10k", "R3 ninp out 10k",
        f"XU1 ninp in vcc 0 out {sub_name}",
        ".options reltol=1e-3 gmin=1e-12",
        ".tran 2u 30m",
        ".meas tran vth_up find v(in) when v(out)=1.75 fall=2",
        ".meas tran vth_dn find v(in) when v(out)=1.75 rise=2",
        ".meas tran out_max max v(out) from=10m to=20m",
        ".meas tran out_min min v(out) from=10m to=20m",
        ".end", "",
    ])


def vca_deck(smooth: bool) -> str:
    """Voltage-controlled amplifier as laws: out = limit(ref + k*V(ctl)*(V(in)-ref)), k = 2/V."""
    law = limit("v(ref)+2*v(ctl)*(v(in)-v(ref))", "0.02", "v(vcc)-1.5", smooth)
    return "\n".join([
        f"* p50 vca {'smooth' if smooth else 'hard'}",
        ".subckt VCA in ctl ref vcc out",
        "rin in ref 1meg", "rctl ctl 0 1meg",
        f"b1 n1 0 v={law}",
        "rout n1 out 50",
        ".ends",
        "VCC vcc 0 DC 5", "VREF ref 0 DC 2.5",
        "VIN in 0 DC 2.5 AC 0.1 SIN(2.5 0.1 1k)",
        "VCTL ctl 0 DC 0 PWL(0 0 10m 5)",
        "XV in ctl ref vcc out VCA", "RL out ref 10k",
        ".options reltol=1e-3 gmin=1e-12",
        ".tran 5u 10m",
        ".meas tran pp_early pp v(out) from=1m to=2m",
        ".meas tran pp_mid pp v(out) from=4.5m to=5.5m",
        ".meas tran out_max max v(out)",
        ".end", "",
    ])


def tf_deck() -> str:
    """H(s) = 1/(1 + s/(Q w0) + s^2/w0^2), fc 1 kHz, Q 0.707, as a controllable-canonical state
    space: x1' = w0 x2, x2' = w0 (u - x1 - x2/Q). Integrators are G-sources (here B currents)
    into C = 1/w0, so each state's current is the bracket; y = x1."""
    w0 = 2 * 3.141592653589793 * 1000
    c = 1 / w0
    return "\n".join([
        "* p50 transfer function",
        ".subckt TF2 in out",
        f"cx1 x1 0 {c:.6e}", f"cx2 x2 0 {c:.6e}",
        "bx1 0 x1 i=v(x2)",
        "bx2 0 x2 i=v(in)-v(x1)-v(x2)/0.707",
        "eout out 0 x1 0 1",
        ".ends",
        "VIN in 0 DC 0 AC 1 SIN(0 1 1k)",
        "XT in out TF2", "RL out 0 10k",
        ".options reltol=1e-3 gmin=1e-12",
        ".ac dec 50 10 100k",
        ".tran 5u 10m",
        ".meas ac g_100 find vm(out) at=100",
        ".meas ac g_1k find vm(out) at=1000",
        ".meas ac g_10k find vm(out) at=10000",
        ".meas ac ph_1k find vp(out) at=1000",
        ".meas tran amp_1k pp v(out) from=5m to=10m",
        ".end", "",
    ])


def functions_deck() -> str:
    """Every function the expression language would emit, evaluated once at the OP."""
    fns = {
        "f_exp": "exp(0.5)", "f_ln": "ln(2)", "f_log10": "log10(50)", "f_sqrt": "sqrt(2)",
        "f_abs": "abs(-1.5)", "f_min": "min(1,2)", "f_max": "max(1,2)", "f_tanh": "tanh(0.5)",
        "f_sin": "sin(0.5)", "f_cos": "cos(0.5)", "f_pow": "pow(2,0.5)", "f_tern": "(1>0.5 ? 3 : 4)",
    }
    lines = ["* p50 functions", "VONE one 0 DC 1"]
    meas = []
    for k, e in fns.items():
        lines.append(f"b{k} {k} 0 v={e}*v(one)")
        lines.append(f"r{k} {k} 0 1k")
        meas.append(f".meas tran {k} avg v({k})")
    return "\n".join([*lines, ".tran 1u 10u", *meas, ".end", ""])


def main(out: str) -> None:
    reqs = []
    for v in ("flat", "mono", "param"):
        reqs.append({"name": f"analyser_{v}", "netlist": analyser_deck(v),
                     "includes": ["models/lm358.lib", "models/lm7805.lib"], "hash": ""})
    for smooth in (False, True):
        tag = "smooth" if smooth else "hard"
        reqs.append({"name": f"schmitt_{tag}", "netlist": schmitt_deck(smooth), "includes": [], "hash": ""})
        reqs.append({"name": f"schmitt_{tag}_windup", "netlist": schmitt_deck(smooth, True), "includes": [], "hash": ""})
        reqs.append({"name": f"vca_{tag}", "netlist": vca_deck(smooth), "includes": [], "hash": ""})
    reqs.append({"name": "schmitt_lm358lib", "netlist": schmitt_deck(False, model="LM358"),
                 "includes": ["models/lm358.lib"], "hash": ""})
    reqs.append({"name": "tf2", "netlist": tf_deck(), "includes": [], "hash": ""})
    reqs.append({"name": "functions", "netlist": functions_deck(), "includes": [], "hash": ""})
    with open(out, "w", encoding="utf-8") as f:
        json.dump(reqs, f, indent=1)
    for r in reqs:
        with open(out.replace("requests.json", f"{r['name']}.cir"), "w", encoding="utf-8") as f:
            f.write(r["netlist"])


if __name__ == "__main__":
    main(sys.argv[1])
