"""The demo circuit from the circuit-core tests (±12 V supply, 1 kHz source, LLD §12 Sallen-Key
low-pass), loaded from its IR snapshot and simulated as compiled."""

import math

from pytest import approx

from circuits import Bench, demo_snapshot, tran
from test_parts import ok, opv

# R = 10k, C1 = 22n, C2 = 12n: f0 = 1 / (2π R sqrt(C1 C2)) ≈ 980 Hz, Q = sqrt(C1 / C2) / 2 ≈ 0.68
F0 = 1 / (2 * math.pi * 10e3 * math.sqrt(22e-9 * 12e-9))
Q = math.sqrt(22e-9 / 12e-9) / 2


def lowpass_db(f: float) -> float:
    x = f / F0
    return -10 * math.log10((1 - x * x) ** 2 + (x / Q) ** 2)


def test_sallen_key_ac_meets_its_spec(reg):
    r = ok(Bench(reg, demo_snapshot()).simulate(extra="""
        .meas ac g_pass find vdb(n_out) at=10
        .meas ac fc when vdb(n_out)=g_pass-3
        .meas ac g_10k find vdb(n_out) at=10k
    """))
    assert r.meas["fc"] == approx(1000, rel=0.10)  # the block's spec: fc_hz 1000 ±10%
    assert r.meas["g_pass"] == approx(0, abs=0.01)  # unity gain
    assert r.meas["g_10k"] == approx(lowpass_db(10e3), abs=0.5)  # 2nd-order roll-off, ≈ -40 dB
    # the whole response follows the ideal transfer function
    freq, out = r.vec("frequency", "ac"), r.vec("v(n_out)", "ac")
    for f, re_, im in zip(freq.data, out.data, out.imag):
        if f <= 20e3:
            assert 10 * math.log10(re_ * re_ + im * im) == approx(lowpass_db(f), abs=0.1), f
    assert abs(opv(r, "v(n_out)")) < 1e-3  # no DC offset at the operating point


def test_sallen_key_transient(reg):
    """The 1 V, 1 kHz input comes out at |H(1 kHz)| ≈ 0.66 V, without clipping."""
    r = ok(Bench(reg, demo_snapshot()).simulate(analyses=[tran(5e-6, 6e-3)], extra="""
        .meas tran vmax max v(n_out) from=3m to=6m
        .meas tran vmin min v(n_out) from=3m to=6m
    """))
    expected = 10 ** (lowpass_db(1e3) / 20)
    assert r.meas["vmax"] == approx(expected, rel=0.02) and r.meas["vmin"] == approx(-expected, rel=0.02)
