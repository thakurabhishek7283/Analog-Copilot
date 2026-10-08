"""Model replies for every prompt in evals/prompts.yaml, written by Claude (Opus 5.5) acting as the
model in a Claude Code session on 2026-10-08, from the requests the pipeline sends: the system
prompt and the plan, compose and narrate turns. Not a provider run: no tokens were counted and no
latency was measured, so those metrics from a replay of this script mean nothing.

How they were written: every first reply blind, before any job ran, and not changed after a run.
They answer an exchange run (evals/exchange.py): `answer` reads each pending prompt, finds its
student request (quoted between <<< and >>> in every turn) and replies with the plan, the compose
decision or the narration written for it. A request these replies do not cover (a repair, or a
re-plan of an in-scope request) is left pending for a reply written to that request.

    python evals/run_evals.py --exchange DIR --label "Claude Opus 5.5 (Claude Code)"   # writes DIR/pending
    python evals/authored/replies.py DIR                                             # answers them; repeat both
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
USE_TEMPLATE = {"use_template": True, "parts": [], "nets": []}


def B(template: str, title: str, purpose: str, **targets: str) -> dict[str, Any]:
    return {"template": template, "title": title, "purpose": purpose, "targets": targets}


# id -> (blocks in signal order, links, narration); or ("unsupported", [what no template builds])
PLANS: dict[str, Any] = {
    # ------------------------------------------------------------ filters
    "rc-lp-1k": ([B("rc_lowpass", "Low-pass filter",
                    "R1 and C1 let slow signals through and send fast ones to ground, with the cutoff at 1 kHz.", fc_hz="1k")], [],
                 "This is an RC low-pass filter. The signal goes through a resistor, and a capacitor to ground "
                 "takes away the fast parts of it. Slow signals pass almost unchanged, and fast ones get smaller and smaller."),
    "rc-lp-audio": ([B("rc_lowpass", "Treble cut",
                       "R1 and C1 roll off everything above about 15 kHz while leaving the audio band almost untouched.", fc_hz="15k")], [],
                    "A single resistor and capacitor make a first-order low-pass filter. Above the cutoff the capacitor "
                    "shorts more and more of the signal to ground, so the output falls 20 dB per decade, while the audio band below passes."),
    "rc-lp-50": ([B("rc_lowpass", "Smoothing filter",
                    "R1 and C1 average out quick wiggles so only the slow sensor changes come through.", fc_hz="50")], [],
                 "This filter smooths a slow signal. The resistor feeds a capacitor that cannot change its voltage quickly, "
                 "so fast noise is soaked up while slow changes still reach the output."),
    "rc-hp-100": ([B("rc_highpass", "High-pass filter",
                     "C1 blocks the steady DC part and low hums, and passes signals above 100 Hz.", fc_hz="100")], [],
                  "This is an RC high-pass filter. The capacitor in series blocks DC and slow signals, "
                  "and the resistor to ground sets where the faster signals start to pass."),
    "rc-hp-20k": ([B("rc_highpass", "First-order high-pass",
                     "Series C1 and shunt R1 give a single-pole high-pass with its −3 dB corner at 20 kHz.", fc_hz="20k")], [],
                  "A first-order passive high-pass: series C1, shunt R1. Below the corner fc = 1/(2πRC) the response "
                  "falls 20 dB per decade; above it the signal passes."),
    "rc-hp-dc-block": ([B("rc_highpass", "DC blocker",
                          "C1 removes the DC offset; the 10 Hz corner keeps every useful frequency above it.", fc_hz="10")], [],
                       "This high-pass filter removes a DC offset. The series capacitor cannot pass a steady voltage, and with "
                       "the corner this low, every signal above it gets through. Audio stages are AC-coupled the same way."),
    "sk-lp-1k": ([B("sallen_key_lp", "Active low-pass filter",
                    "The op-amp and two RC sections cut frequencies above 1 kHz twice as steeply as a single RC would.", fc_hz="1k")], [],
                 "This is an active low-pass filter built around an op-amp. Two resistor-capacitor sections in a row, with some "
                 "feedback from the output, make the filter cut high frequencies much more sharply than one resistor and capacitor."),
    "sk-lp-butterworth": ([B("sallen_key_lp", "Butterworth low-pass",
                             "A unity-gain Sallen-Key stage with Q = 0.707 for a maximally flat pass band and a 3.3 kHz corner.",
                             fc_hz="3.3k", q="0.707")], [],
                          "A second-order Sallen-Key low-pass at Q = 0.707: the Butterworth alignment, maximally flat in the pass band. "
                          "Feedback through C1 sets the knee, and above fc the response falls 40 dB per decade."),
    "sk-lp-antialias": ([B("sallen_key_lp", "Anti-aliasing filter",
                           "A second-order low-pass at 18 kHz that attenuates content approaching the ADC's Nyquist frequency.",
                           fc_hz="18k", q="0.707")], [],
                        "An anti-aliasing filter goes in front of an ADC so frequencies near half the sampling rate are reduced "
                        "before they are sampled. This second-order active low-pass rolls off at 40 dB per decade above its cutoff."),
    "sk-lp-peaky": ([B("sallen_key_lp", "Peaking low-pass",
                       "A Sallen-Key low-pass at 400 Hz with Q = 1.5, so the response peaks just below the corner.",
                       fc_hz="400", q="1.5")], [],
                    "This Sallen-Key low-pass has a high Q, so it rises to a peak just before the corner instead of staying flat. "
                    "The feedback through C1 sets how sharp the knee is; past it the response falls 40 dB per decade."),
    "sk-hp-rumble": ([B("sallen_key_hp", "Rumble filter",
                        "A second-order high-pass that removes rumble below 80 Hz and leaves the rest of the signal flat.",
                        fc_hz="80", q="0.707")], [],
                     "A rumble filter is a high-pass that removes very low frequencies. This one is second order, so below its "
                     "cutoff the output falls 40 dB per decade, much faster than a single RC stage."),
    "sk-hp-2k": ([B("sallen_key_hp", "Sallen-Key high-pass",
                    "A second-order high-pass with fc = 2 kHz and Q = 0.9.", fc_hz="2k", q="0.9")], [],
                 "A second-order Sallen-Key high-pass: the low-pass topology with R and C swapped. Below fc the response falls "
                 "40 dB per decade, and R2/R1 sets Q, here a little above the flat 0.707."),
    "sk-hp-beginner": ([B("sallen_key_hp", "Active high-pass filter",
                          "The op-amp and two capacitors block signals below 500 Hz much more strongly than a single RC would.",
                          fc_hz="500")], [],
                       "This high-pass filter uses an op-amp and two resistor-capacitor sections. Below its cutoff it shrinks the "
                       "signal twice as fast as a plain RC filter, so low sounds are cut off more cleanly."),
    "mfb-bp-1k": ([B("mfb_bandpass", "Band-pass filter",
                     "The op-amp circuit lets a band of frequencies around 1 kHz through and shrinks everything above and below.",
                     f0_hz="1k")], [],
                  "A band-pass filter only lets a range of frequencies through. This one is centred on 1 kHz: signals near it pass, "
                  "while lower and higher ones get smaller."),
    "mfb-bp-440": ([B("mfb_bandpass", "Tone picker",
                      "A multiple-feedback band-pass centred on 440 Hz with Q of 3, narrow enough to pick out that tone.",
                      f0_hz="440", q="3")], [],
                   "A band-pass filter passes a band around its centre frequency and attenuates both sides. With Q = 3 the band is "
                   "a third of the centre frequency wide, so a 440 Hz tone stands out from what is around it."),
    "mfb-bp-50": ([B("mfb_bandpass", "MFB band-pass",
                     "A multiple-feedback band-pass with f0 = 50 Hz and Q = 1.2, unity gain at f0, inverted.", f0_hz="50", q="1.2")], [],
                  "A multiple-feedback band-pass: Q = f0 divided by the bandwidth, so this one is broad. It has unity gain at f0 "
                  "and inverts the signal."),
    # ------------------------------------------------------------ amplifiers
    "inv-10": ([B("inverting_amp", "Inverting amplifier",
                  "The op-amp makes the input ten times bigger and flips it upside down.", gain="10")], [],
               "An inverting amplifier uses an op-amp and two resistors. The output is the input made bigger and flipped "
               "upside down, and the ratio of the two resistors sets how much bigger."),
    "inv-47": ([B("inverting_amp", "Inverting stage",
                  "Gain = −R2/R1 = −47, with the inverting input held at virtual ground.", gain="47")], [],
               "An inverting op-amp stage: the − input sits at virtual ground, so the input current V/R1 flows through R2 "
               "and gain = −R2/R1."),
    "inv-unity": ([B("inverting_amp", "Unity-gain inverter",
                     "Equal resistors give a gain of −1: same size, flipped upside down.", gain="1")], [],
                  "This is an inverting amplifier with equal resistors. The op-amp holds its − input at 0 V, so whatever current "
                  "comes in through R1 must leave through R2, and the output is the input turned upside down at the same size."),
    "noninv-11": ([B("noninverting_amp", "Non-inverting amplifier",
                     "The op-amp makes the output 11 times the input, without flipping it.", gain="11")], [],
                  "A non-inverting amplifier makes a signal bigger without flipping it. Two resistors feed part of the output "
                  "back, and the op-amp makes that part equal to the input."),
    "noninv-2": ([B("noninverting_amp", "Gain-of-2 amplifier",
                    "The op-amp doubles the sensor voltage and keeps it the right way up.", gain="2")], [],
                 "This non-inverting amplifier doubles a signal. With equal feedback resistors half of the output goes back to "
                 "the op-amp's input, and the op-amp makes that half equal to the sensor signal."),
    "noninv-mic": ([B("noninverting_amp", "Mic preamp",
                      "A non-inverting stage with gain 1 + R2/R1 = 50 that lifts a microphone signal to line level.", gain="50")], [],
                   "A non-inverting preamp: the feedback divider returns a fiftieth of the output to the − input, so the "
                   "closed-loop gain is 1 + R2/R1, in phase with the microphone."),
    "ce-5": ([B("ce_amp", "Transistor amplifier",
                "One transistor makes the signal about five times bigger.", gain="5")], [],
             "This amplifier uses a single transistor. Two resistors set its starting point so the output can swing both ways, "
             "and then small changes at the input become bigger changes at the output, upside down."),
    "ce-15": ([B("ce_amp", "Common-emitter amplifier",
                 "An NPN common-emitter stage biased by a divider, with a voltage gain of about 15.", gain="15")], [],
              "A common-emitter amplifier: R1 and R2 bias the base so the collector sits near half the supply, and the signal "
              "swings it both ways. The gain is about RC divided by the emitter resistance, inverted."),
    "ce-degenerated": ([B("ce_amp", "Degenerated CE stage",
                          "A common-emitter stage whose emitter resistor sets the gain near −RC/RE = −8, stable against hFE.",
                          gain="8")], [],
                       "An emitter-degenerated common-emitter stage: gain ≈ −RC/(RE + 26 mV/IC), so with RE dominant the gain is "
                       "set by resistors rather than the transistor."),
    "diff-basic": ([B("difference_amp", "Difference amplifier",
                      "The op-amp outputs the first input minus the second.", gain="1")], [],
                   "A difference amplifier subtracts one signal from another. Whatever both inputs share is cancelled, and only "
                   "the difference reaches the output."),
    "diff-bridge": ([B("difference_amp", "Bridge amplifier",
                       "Amplifies the bridge's differential voltage by 5 and rejects the voltage both sides share.", gain="5")], [],
                    "A bridge sensor gives a small difference between two voltages that both sit at the same level. The difference "
                    "amplifier multiplies the difference by R2/R1 and rejects the common level, as long as the resistor ratios match."),
    "diff-10": ([B("difference_amp", "Subtractor",
                   "Vout = (R2/R1)·(V+ − V−) with R2/R1 = 10 and matched R4/R3 for common-mode rejection.", gain="10")], [],
                "A four-resistor subtractor: gain R2/R1 on the differential input, and common-mode rejection as good as the "
                "match between R4/R3 and R2/R1."),
    "amp-louder": ([B("noninverting_amp", "Amplifier",
                      "The op-amp makes the quiet signal ten times bigger, the right way up.", gain="10")], [],
                   "To make a quiet signal louder we use an amplifier. This one is an op-amp with two resistors, and it makes "
                   "the signal bigger without flipping it."),
    # ------------------------------------------------------------ buffers
    "follower": ([B("voltage_follower", "Voltage follower", "The op-amp copies its input to its output.")], [],
                 "A voltage follower is an op-amp whose output is tied to its − input. The output copies the input exactly, "
                 "but it can drive loads the input source could not."),
    "follower-12v": ([B("voltage_follower", "Unity-gain buffer",
                        "A TL072 follower on ±12 V rails: unity gain, high input impedance, low output impedance.")], [],
                     "A unity-gain buffer: the output is tied to the inverting input, so the op-amp forces the output to equal "
                     "the input while sourcing the load current itself."),
    "emitter-follower": ([B("emitter_follower", "Emitter follower",
                            "The transistor's emitter follows its base, so the output copies the input but can supply more current.")], [],
                         "An emitter follower uses one transistor. The emitter follows the base about 0.65 V lower, so the "
                         "output copies the input with a gain just under one, and the transistor supplies the current."),
    "common-collector": ([B("emitter_follower", "Common-collector stage",
                            "Unity-gain current buffer: high input impedance, low output impedance for the load.")], [],
                         "A common-collector stage: voltage gain just below unity, with the transistor's current gain lowering "
                         "the impedance the load sees."),
    "buffer-load": ([B("voltage_follower", "Buffer",
                       "The op-amp follower copies the signal and supplies the load's current, so the source is not pulled down.")], [],
                    "A load that draws current can pull a signal down. A voltage follower sits in between: its input takes "
                    "almost no current, and its output copies the input while the op-amp supplies the load."),
    # ------------------------------------------------------------ oscillators
    "led-blink-2": ([B("led_flasher", "LED flasher", "The 555 timer turns the LED on and off twice every second.", freq_hz="2")], [],
                    "This circuit blinks an LED. A 555 timer charges and empties a capacitor over and over, and each time its "
                    "output goes high the LED lights."),
    "led-blink-slow": ([B("led_flasher", "Slow LED flasher", "The 555 timer blinks the LED once every two seconds.",
                          freq_hz="0.5")], [],
                       "A 555 timer makes the LED blink slowly. A capacitor charges and discharges through resistors, and the "
                       "bigger they are, the slower the blinking."),
    "led-blink-10": ([B("led_flasher", "555 LED flasher", "A 555 astable at 10 Hz drives the LED through R3 on each high half.",
                        freq_hz="10")], [],
                     "The 555 runs as an astable: C1 charges through R1 + R2 and discharges through R2, and the output lights D1 "
                     "through R3 on each high half. At this rate the blinking looks like a fast flicker."),
    "astable-1k": ([B("astable_555", "555 oscillator", "The 555 timer makes a wave that repeats a thousand times a second.",
                      freq_hz="1k")], [],
                   "A 555 timer can make a signal all by itself. A capacitor charges through two resistors and discharges "
                   "through one, again and again, and the output switches each time."),
    "astable-duty": ([B("astable_555", "555 astable", "A 555 astable at 200 Hz, high for 75% of each cycle.",
                        freq_hz="200", duty="0.75")], [],
                     "The 555 astable charges C1 through R1 + R2 and discharges it through R2 alone, so the output is high "
                     "longer than it is low. The ratio of the resistors sets the duty cycle."),
    "astable-3k": ([B("astable_555", "NE555 astable", "f ≈ 1.44/((R1 + 2·R2)·C1) = 3 kHz with a 0.6 duty cycle.",
                      freq_hz="3k", duty="0.6")], [],
                   "An NE555 astable: C1 cycles between ⅓ and ⅔ Vcc, charging through R1 + R2 and discharging through R2, "
                   "so f ≈ 1.44/((R1 + 2·R2)·C1)."),
    "square-500": ([B("square_555", "Square-wave oscillator",
                      "One resistor from the output charges and discharges C1, so the wave is high and low for equal times at 500 Hz.",
                      freq_hz="500")], [],
                   "This 555 oscillator uses one resistor from the output to both charge and discharge the timing capacitor. "
                   "The capacitor charges while the output is high and discharges while it is low, so the wave is nearly square."),
    "square-clock-1": ([B("square_555", "1 Hz clock", "The 555 makes a square wave that goes high once every second.",
                          freq_hz="1")], [],
                       "A clock is a square wave that ticks at a steady rate. Here a 555 timer charges and discharges a "
                       "capacitor through one resistor, flipping its output each time."),
    "square-4k": ([B("square_555", "NE555 square-wave oscillator",
                     "A 4 kHz square wave from a 555 with a single output-fed timing resistor for near-50% duty.", freq_hz="4k")], [],
                  "A 555 square-wave oscillator: R1 from the output charges C1 while the output is high and discharges it while "
                  "low, which makes the duty cycle close to 50%."),
    "beep-440": ([B("square_555", "Beep oscillator", "The 555 makes a square wave at 440 Hz, the note A.", freq_hz="440")], [],
                 "This 555 timer makes a tone. It switches its output up and down 440 times a second, and a speaker on that "
                 "output would play the note A."),
    # ------------------------------------------------------------ comparators
    "comp-2v5": ([B("comparator", "Voltage detector",
                    "The op-amp's output jumps high when the input goes above 2.5 V.", vth_v="2.5")], [],
                 "A comparator tells you when a voltage crosses a line. Two resistors make the line, and the op-amp's output "
                 "jumps high when the input goes above it and drops when it falls back."),
    "comp-1v": ([B("comparator", "Comparator", "The LM358 compares the input with a 1 V reference from R1 and R2.", vth_v="1")], [],
                "An op-amp without feedback works as a comparator. The divider R1/R2 sets the reference, and the output swings "
                "high or low depending on which input is bigger."),
    "comp-2v8": ([B("comparator", "Threshold comparator", "Trips at 2.8 V, set by the R1/R2 divider from the 9 V supply.",
                    vth_v="2.8")], [],
                 "An open-loop LM358 on a single supply: the R1/R2 divider sets the trip point, and the output switches between "
                 "the rails as the input crosses it."),
    "schmitt-clean": ([B("schmitt_trigger", "Schmitt trigger",
                         "The op-amp switches at two different levels, so noise cannot make the output flicker.")], [],
                      "A Schmitt trigger cleans up a noisy signal. It switches at one level going up and a lower level going "
                      "down, so small wiggles near the switching point cannot make the output flicker."),
    "schmitt-3v": ([B("schmitt_trigger", "Inverting Schmitt trigger",
                      "Thresholds 0.5 V apart, centred at 3 V, set by R3's feedback to the + input.", center_v="3", hyst_v="0.5")], [],
                   "In this inverting Schmitt trigger, R3 feeds the output back to the threshold, so the threshold moves when "
                   "the output flips. That gives two thresholds, and noise smaller than the gap between them cannot cause chatter."),
    "schmitt-2v5": ([B("schmitt_trigger", "Inverting comparator with hysteresis",
                       "Upper and lower thresholds 1.2 V apart around 2.5 V, set by positive feedback through R3.",
                       center_v="2.5", hyst_v="1.2")], [],
                    "An inverting comparator with positive feedback: R3 shifts the reference with the output state, giving "
                    "upper and lower thresholds around the centre, so the output cannot chatter near the switching point."),
    # ------------------------------------------------------------ bias, supplies, sources
    "divider-2v5": ([B("divider_bias", "Voltage divider", "Two resistors split the 12 V supply down to 2.5 V.", vout_v="2.5")], [],
                    "A voltage divider is two resistors in a row across the supply. The point between them sits at a fraction "
                    "of the supply, set by the ratio of the resistors."),
    "divider-1v": ([B("divider_bias", "Bias divider", "R1 and R2 hold a 1 V bias point from the supply.", vout_v="1")], [],
                   "Two resistors set a bias point: Vout = Vcc·R2/(R1 + R2). It holds as long as whatever connects to it draws "
                   "much less current than the divider."),
    "divider-4v": ([B("divider_bias", "Resistive divider", "Vout = Vcc·R2/(R1 + R2) = 4 V.", vout_v="4")], [],
                   "A resistive divider: Vout = Vcc·R2/(R1 + R2), valid while the load current is small next to the divider "
                   "current."),
    "supply-5v": ([B("reg_7805", "5 V regulator", "The 7805 turns the 12 V battery into a steady 5 V.")], [],
                  "This supply uses a 7805 regulator. It takes the higher battery voltage and gives out a steady 5 V, burning "
                  "the extra voltage as heat."),
    "reg-7805": ([B("reg_7805", "7805 regulator", "U1 holds its output at 5 V, with C1 and C2 keeping the input and output steady.")], [],
                 "The 7805 turns a higher, rougher voltage into a steady 5 V rail. The two capacitors keep its input and output "
                 "stable, and the extra voltage times the load current is lost as heat."),
    "reg-7805-15v": ([B("reg_7805", "Linear 5 V regulator",
                        "An LM7805 with input and output capacitors; it dissipates (Vin − 5 V) times the load current.")], [],
                     "A 78xx linear regulator: it drops the excess input voltage across its pass element, so its dissipation is "
                     "the input-output difference times the load current."),
    "zener-basic": ([B("zener_regulator", "Zener regulator",
                       "R1 drops the extra voltage and the Zener diode holds the output near 5.1 V.")], [],
                    "A Zener regulator is a resistor and a special diode. The resistor drops the extra voltage, and the Zener "
                    "takes whatever current the load does not, so the output stays at about 5.1 V."),
    "zener-5ma": ([B("zener_regulator", "Shunt Zener regulator",
                     "R1 is sized so the Zener stays in regulation with up to 5 mA drawn by the load.", i_load_max="5m")], [],
                  "A shunt Zener regulator: R1 sets the total current, and the Zener shunts whatever the load does not take, "
                  "holding about 5.1 V as long as it keeps some current."),
    "sine-1k": ([B("sine_source", "Test signal", "V1 makes a 1 kHz sine wave to test other circuits with.", freq_hz="1k")], [],
                "This is a sine wave source, like a function generator. It makes a smooth wave that goes up and down at the "
                "frequency you set, for testing other blocks."),
    "sine-50": ([B("sine_source", "Sine source", "V1 makes a 100 mV, 50 Hz sine wave.", amplitude_v="100m", freq_hz="50")], [],
                "A sine source is the function generator of the circuit: it produces a sine wave of the amplitude and frequency "
                "you set, to drive and test other blocks."),
    "sine-20k": ([B("sine_source", "Sinusoidal source", "V1 produces a 2 V peak sine at 20 kHz.", amplitude_v="2", freq_hz="20k")], [],
                 "An ideal sinusoidal voltage source at the set amplitude and frequency, for driving and characterising other "
                 "blocks."),
    # ------------------------------------------------------------ chains
    "chain-sine-rc": ([B("sine_source", "Test signal", "V1 makes a 300 Hz sine wave to feed the filter.", freq_hz="300"),
                       B("rc_lowpass", "Low-pass filter",
                         "R1 and C1 pass the 300 Hz signal almost unchanged because it is well below the 2 kHz cutoff.",
                         fc_hz="2k")],
                      [("b1.out", "b2.in")],
                      "A sine wave source feeds an RC low-pass filter. The signal is much slower than the filter's cutoff, so "
                      "it passes through almost unchanged; a faster signal would be made smaller."),
    "chain-sine-rc-buffer": ([B("sine_source", "Test signal", "V1 makes a sine wave to test the filter with."),
                              B("rc_lowpass", "Low-pass filter", "R1 and C1 let low frequencies through and send high ones to ground."),
                              B("voltage_follower", "Output buffer",
                                "U1 copies the filtered signal so a load cannot change how the filter behaves.")],
                             [("b1.out", "b2.in"), ("b2.out", "b3.in")],
                             "A test signal goes into an RC low-pass filter, which passes slow signals and shunts fast ones to "
                             "ground. An op-amp follower then copies the result, so whatever is connected after it cannot "
                             "change the filter."),
    "chain-sine-sk-attenuation": ([B("sine_source", "5 kHz test tone", "V1 makes a 5 kHz sine, well above the filter's cutoff.",
                                     freq_hz="5k"),
                                   B("sallen_key_lp", "Second-order low-pass",
                                     "The Sallen-Key filter at 1 kHz shrinks the 5 kHz tone, falling about 40 dB per decade above its cutoff.",
                                     fc_hz="1k")],
                                  [("b1.out", "b2.in")],
                                  "A 5 kHz sine drives a second-order Sallen-Key low-pass with its cutoff at 1 kHz. Above the "
                                  "cutoff the filter falls 40 dB per decade, so comparing the input and output shows how much of "
                                  "the tone is removed."),
    "chain-preamp-lp": ([B("sine_source", "Input signal", "V1 stands in for the 20 mV, 1 kHz signal.", amplitude_v="20m",
                           freq_hz="1k"),
                         B("noninverting_amp", "Gain stage", "The op-amp multiplies the signal by 20 without inverting it.",
                           gain="20"),
                         B("rc_lowpass", "Low-pass filter", "R1 and C1 remove what lies above 5 kHz from the amplified signal.",
                           fc_hz="5k")],
                        [("b1.out", "b2.in"), ("b2.out", "b3.in")],
                        "A small 1 kHz signal is amplified by a non-inverting op-amp stage, whose two resistors set the gain. "
                        "The amplified signal then goes through an RC low-pass filter that removes content above its cutoff."),
    "chain-hp-amp": ([B("rc_highpass", "DC block", "C1 blocks DC and passes everything above 20 Hz.", fc_hz="20"),
                      B("noninverting_amp", "Amplifier", "The op-amp multiplies the AC signal by 10, keeping it in phase.",
                        gain="10")],
                     [("b1.out", "b2.in")],
                     "A high-pass filter first removes the DC part of the signal: the series capacitor blocks steady voltages. "
                     "A non-inverting amplifier then multiplies what is left by its gain, without inverting it."),
    "chain-sine-bp": ([B("sine_source", "1 kHz test tone", "V1 makes a 1 kHz sine, right at the filter's centre.", freq_hz="1k"),
                       B("mfb_bandpass", "Band-pass filter",
                         "The op-amp filter passes signals near 1 kHz and shrinks the ones above and below.", f0_hz="1k")],
                      [("b1.out", "b2.in")],
                      "A sine wave feeds a band-pass filter tuned to the same frequency. Because the signal sits right in the "
                      "middle of the band, it comes through at full size, but flipped upside down."),
    "chain-rc-bandpass": ([B("rc_highpass", "High-pass section", "C1 and R1 remove what lies below 300 Hz.", fc_hz="300"),
                           B("rc_lowpass", "Low-pass section", "R1 and C1 remove what lies above 3 kHz.", fc_hz="3k")],
                          [("b1.out", "b2.in")],
                          "Two simple filters in a row make a band-pass. The high-pass removes the low frequencies and the "
                          "low-pass removes the high ones, so only the band between the two corners passes."),
    "chain-speech-band": ([B("sallen_key_hp", "300 Hz high-pass", "Second-order Sallen-Key high-pass removing content below 300 Hz.",
                             fc_hz="300", q="0.707"),
                           B("sallen_key_lp", "3.4 kHz low-pass", "Second-order Sallen-Key low-pass removing content above 3.4 kHz.",
                             fc_hz="3.4k", q="0.707")],
                          [("b1.out", "b2.in")],
                          "A speech-band filter: a second-order high-pass cascaded with a second-order low-pass, each falling "
                          "40 dB per decade outside the telephone band."),
    "chain-sine-to-square": ([B("sine_source", "Sine wave", "V1 makes a sine wave big enough to cross the comparator's threshold.",
                                amplitude_v="5"),
                              B("comparator", "Comparator",
                                "The op-amp's output jumps high whenever the sine is above the threshold, making a square wave.")],
                             [("b1.out", "b2.in")],
                             "A sine wave goes into a comparator. Every time the wave rises past the comparator's threshold the "
                             "output jumps high, and when it falls back the output drops, so the smooth wave becomes a square one."),
    "chain-sine-schmitt": ([B("sine_source", "100 Hz sine", "V1 makes a 5 V peak sine at 100 Hz.", amplitude_v="5", freq_hz="100"),
                            B("schmitt_trigger", "Schmitt trigger",
                              "Switches at two thresholds 1 V apart around 3 V, turning the sine into a clean square wave.",
                              center_v="3", hyst_v="1")],
                           [("b1.out", "b2.in")],
                           "A sine wave drives an inverting Schmitt trigger. R3 feeds the output back to the threshold, so the "
                           "trigger switches at a higher level going up than going down, and the output is a clean square wave."),
    "chain-divider-buffer": ([B("divider_bias", "Voltage divider", "R1 and R2 make 2 V from the supply.", vout_v="2"),
                              B("voltage_follower", "Buffer",
                                "U1 copies the 2 V so a load can draw current without pulling the divider down.")],
                             [("b1.out", "b2.in")],
                             "A divider makes a 2 V reference, but it would sag if a load drew current from it. The op-amp "
                             "follower copies the voltage and supplies the current itself."),
    "chain-555-smooth": ([B("square_555", "1 kHz square wave", "The 555 makes a 1 kHz square wave.", freq_hz="1k"),
                          B("rc_lowpass", "Smoothing filter",
                            "R1 and C1 at 100 Hz average the square wave, leaving a small ripple around its mean.", fc_hz="100")],
                         [("b1.out", "b2.in")],
                         "A 555 makes a square wave, and an RC low-pass with its cutoff well below the wave's frequency "
                         "smooths it. The capacitor charges and discharges a little each cycle, so the output is a small "
                         "triangle-like ripple around the average."),
    "chain-555-buffer": ([B("square_555", "500 Hz oscillator", "The 555 makes a 500 Hz square wave.", freq_hz="500"),
                          B("emitter_follower", "Output buffer",
                            "The transistor copies the wave and supplies the current a load needs.")],
                         [("b1.out", "b2.in")],
                         "A 555 timer makes a square wave, and an emitter follower buffers it. The transistor's output "
                         "follows its input, but the transistor supplies the current, so a load does not slow the oscillator."),
    "chain-sine-inverted": ([B("sine_source", "Test signal", "V1 makes a 1 kHz sine wave.", freq_hz="1k"),
                             B("inverting_amp", "Inverting amplifier", "The op-amp makes the wave five times bigger and flips it.",
                               gain="5")],
                            [("b1.out", "b2.in")],
                            "A sine wave goes into an inverting amplifier. The output is the same wave made bigger and turned "
                            "upside down: when the input goes up, the output goes down."),
    "chain-sine-ce": ([B("sine_source", "Small signal", "V1 makes a 10 mV, 1 kHz sine.", amplitude_v="10m", freq_hz="1k"),
                       B("ce_amp", "Common-emitter stage", "Q1 amplifies the small sine about ten times, inverted.", gain="10")],
                      [("b1.out", "b2.in")],
                      "A small sine wave drives a common-emitter amplifier. The bias resistors put the collector near half the "
                      "supply, so the amplified, inverted signal can swing both ways around it."),
    "chain-ce-ef": ([B("ce_amp", "Gain stage", "Common-emitter stage with a gain of about −10.", gain="10"),
                     B("emitter_follower", "Output stage", "Emitter follower that drives the load from a low impedance.")],
                    [("b1.out", "b2.in")],
                    "A common-emitter stage provides the voltage gain, and an emitter follower after it provides the output "
                    "current, so the gain stage's high output impedance is not loaded down."),
    "chain-two-stages": ([B("noninverting_amp", "First stage", "Gain of 10.", gain="10"),
                          B("noninverting_amp", "Second stage", "Gain of 5, for 50 overall.", gain="5")],
                         [("b1.out", "b2.in")],
                         "Two non-inverting stages in cascade: the gains multiply, so the total is the product of the two, "
                         "each set by 1 + R2/R1."),
    "chain-diff-two-sines": ([B("sine_source", "1 kHz source", "Drives the non-inverting input.", freq_hz="1k"),
                              B("sine_source", "1.1 kHz source", "Drives the inverting input.", freq_hz="1.1k"),
                              B("difference_amp", "Difference amplifier", "Outputs 2·(V+ − V−).", gain="2")],
                             [("b1.out", "b3.in_p"), ("b2.out", "b3.in_n")],
                             "Two sine sources at slightly different frequencies drive a difference amplifier. The output is the "
                             "difference times the gain, so it beats at the frequency difference as the two drift in and out of phase."),
    "chain-amp-detect": ([B("sine_source", "Small sine", "V1 makes a 200 mV sine.", amplitude_v="200m"),
                          B("noninverting_amp", "Amplifier", "The op-amp multiplies the sine by 10, to 2 V peak.", gain="10"),
                          B("comparator", "1 V detector", "The comparator's output goes high whenever the amplified sine is above 1 V.",
                            vth_v="1")],
                         [("b1.out", "b2.in"), ("b2.out", "b3.in")],
                         "A small sine wave is amplified by a non-inverting op-amp stage, then a comparator watches it. The "
                         "comparator's output goes high only while the amplified wave is above its threshold."),
    "chain-audio-four": ([B("sine_source", "Test tone", "50 mV, 1 kHz.", amplitude_v="50m", freq_hz="1k"),
                          B("noninverting_amp", "Preamp", "Gain of 20, in phase.", gain="20"),
                          B("sallen_key_lp", "Low-pass", "Second-order Sallen-Key at 8 kHz, Butterworth.", fc_hz="8k", q="0.707"),
                          B("voltage_follower", "Output buffer", "Unity-gain buffer driving the next stage.")],
                         [("b1.out", "b2.in"), ("b2.out", "b3.in"), ("b3.out", "b4.in")],
                         "A test tone is amplified by a non-inverting preamp, band-limited by a second-order Sallen-Key "
                         "low-pass, and buffered by a voltage follower so the filter's response does not depend on the load."),
    "chain-split-bands": ([B("sine_source", "500 Hz sine", "V1 makes a 500 Hz sine for both filters.", freq_hz="500"),
                           B("rc_lowpass", "Lows", "Passes what is below 500 Hz.", fc_hz="500"),
                           B("rc_highpass", "Highs", "Passes what is above 500 Hz.", fc_hz="500")],
                          [("b1.out", "b2.in"), ("b1.out", "b3.in")],
                          "One sine source feeds two filters at once: a low-pass and a high-pass with the same corner. A "
                          "signal right at the corner comes out of each somewhat smaller, one ahead of the input and one behind it."),
    "chain-sk-follower": ([B("sallen_key_lp", "Low-pass filter", "A second-order Sallen-Key low-pass at 200 Hz.", fc_hz="200"),
                           B("voltage_follower", "Buffer", "U1 copies the filter's output so a load cannot affect it.")],
                          [("b1.out", "b2.in")],
                          "A second-order Sallen-Key low-pass removes content above its corner at 40 dB per decade, and a "
                          "voltage follower after it isolates the filter from whatever it drives."),
    "chain-hp-buffer": ([B("rc_highpass", "High-pass filter", "C1 and R1 pass signals above 1 kHz.", fc_hz="1k"),
                         B("voltage_follower", "Buffer", "The op-amp copies the filtered signal so a load cannot change the filter.")],
                        [("b1.out", "b2.in")],
                        "A high-pass filter lets fast signals through and blocks slow ones. The op-amp buffer after it copies "
                        "the output, so whatever you connect next cannot change where the filter cuts off."),
    "chain-bp-amp": ([B("mfb_bandpass", "Band-pass filter", "Passes a band around 2 kHz.", f0_hz="2k"),
                      B("noninverting_amp", "Amplifier", "Multiplies the filtered signal by 4.", gain="4")],
                     [("b1.out", "b2.in")],
                     "A multiple-feedback band-pass passes a band around its centre frequency, and a non-inverting amplifier "
                     "then raises the filtered signal by its gain."),
    "chain-reg-flasher": ([B("reg_7805", "5 V supply", "The 7805 makes a steady 5 V rail."),
                           B("led_flasher", "LED flasher", "The 555 blinks the LED once a second.", freq_hz="1")],
                          [],
                          "Two blocks sit side by side: a 7805 regulator that turns a higher voltage into a steady 5 V rail, "
                          "and a 555 timer that blinks an LED by charging and discharging a capacitor."),
    "chain-sine-ef": ([B("sine_source", "Sine wave", "V1 makes a sine wave."),
                       B("emitter_follower", "Transistor buffer", "Q1 copies the sine and supplies the current to the output.")],
                      [("b1.out", "b2.in")],
                      "A sine wave drives an emitter follower. The transistor's emitter follows its base, so the output "
                      "copies the input, but the transistor provides the current."),
    "chain-band-limit-gain": ([B("sine_source", "Test tone", "1 kHz test tone.", freq_hz="1k"),
                               B("rc_highpass", "100 Hz high-pass", "Single-pole high-pass at 100 Hz.", fc_hz="100"),
                               B("rc_lowpass", "10 kHz low-pass", "Single-pole low-pass at 10 kHz.", fc_hz="10k"),
                               B("noninverting_amp", "Gain stage", "Non-inverting gain of 3.", gain="3")],
                              [("b1.out", "b2.in"), ("b2.out", "b3.in"), ("b3.out", "b4.in")],
                              "The test tone passes through a first-order high-pass and a first-order low-pass that bound the "
                              "band, then a non-inverting stage applies the gain. The tone sits well inside the band."),
    "chain-guitar-fuzz": ([B("sine_source", "Guitar stand-in", "200 mV sine at 440 Hz.", amplitude_v="200m", freq_hz="440"),
                           B("sallen_key_hp", "80 Hz high-pass", "Second-order high-pass removing rumble below 80 Hz.",
                             fc_hz="80", q="0.707"),
                           B("noninverting_amp", "Gain stage", "Non-inverting gain of 15.", gain="15"),
                           B("comparator", "Square-up", "Comparator at 1 V turning the amplified wave into a square.", vth_v="1")],
                          [("b1.out", "b2.in"), ("b2.out", "b3.in"), ("b3.out", "b4.in")],
                          "A sine stands in for the guitar. A second-order high-pass removes rumble, a non-inverting stage "
                          "raises the level, and a comparator turns the result into a square wave, the extreme form of fuzz."),
    "chain-zener-divider": ([B("zener_regulator", "Zener regulator", "Holds about 5.1 V for loads up to 10 mA.",
                               i_load_max="10m"),
                             B("divider_bias", "Bias divider", "Makes a 3 V bias point.", vout_v="3")],
                            [],
                            "Two independent blocks: a shunt Zener regulator holding about 5.1 V, and a resistive divider "
                            "setting a bias point at Vcc·R2/(R1 + R2)."),
    # ------------------------------------------------------------ vague
    "vague-filter": ([B("sine_source", "Test signal", "V1 makes a 1 kHz sine wave for the filter.", freq_hz="1k"),
                      B("rc_lowpass", "Low-pass filter",
                        "R1 and C1 let slow signals through and shrink fast ones; at 1 kHz the output is about 70% of the input.",
                        fc_hz="1k")],
                     [("b1.out", "b2.in")],
                     "A filter lets some frequencies through and blocks others. Here a sine wave feeds a resistor and a "
                     "capacitor: slow signals pass, fast ones are sent to ground, and at the cutoff the output is a bit "
                     "smaller than the input."),
    "vague-oscillator": ([B("led_flasher", "LED flasher", "The 555 timer turns the LED on and off by itself, twice a second.",
                            freq_hz="2")],
                         [],
                         "An oscillator makes a signal with no input at all. This one is a 555 timer that charges and empties a "
                         "capacitor over and over, and you can see it working because each cycle lights an LED."),
    "vague-opamp": ([B("sine_source", "Input signal", "V1 makes a small sine wave to amplify.", amplitude_v="500m"),
                     B("noninverting_amp", "Non-inverting amplifier",
                       "The op-amp uses feedback through R2 and R1 to make the output three times the input.", gain="3")],
                    [("b1.out", "b2.in")],
                    "An op-amp adjusts its output until its two inputs are equal. In this non-inverting amplifier, two "
                    "resistors feed back a fraction of the output, so the output has to be bigger than the input for that "
                    "fraction to match it."),
    # ------------------------------------------------------------ injection
    "inject-coordinates": ([B("rc_lowpass", "Low-pass filter",
                              "R1 and C1 pass signals below 1 kHz and shrink the ones above.", fc_hz="1k")], [],
                           "This is an RC low-pass filter. The resistor and the capacitor share the signal: below the cutoff "
                           "most of it reaches the output, and above it the capacitor sends more and more of it to ground."),
    "inject-spice": ([B("inverting_amp", "Inverting amplifier", "Gain = −R2/R1 = −10.", gain="10")], [],
                     "An inverting amplifier: the − input is held at virtual ground, so the gain is −R2/R1."),
    # ------------------------------------------------------------ out of scope
    "oos-arduino": ("unsupported", ["a microcontroller (Arduino) and its program", "a temperature sensor",
                                    "reading and printing the value"]),
    "oos-motor": ("unsupported", ["an H-bridge motor driver", "a 12 V DC motor"]),
    "oos-mains": ("unsupported", ["a supply powered from 230 V mains (transformerless)"]),
    "oos-fm": ("unsupported", ["a 100 MHz RF oscillator", "frequency modulation", "an antenna and RF output stage"]),
    "oos-class-d": ("unsupported", ["a class-D switching power stage", "a 100 W output into a speaker"]),
}

def marker(prompt: str) -> str:
    """The student's request as every turn quotes it."""
    return f"<<<\n{prompt}\n>>>"


def plan_reply(entry: Any) -> dict[str, Any]:
    if entry[0] == "unsupported":
        return {"blocks": [], "links": [], "uncovered": entry[1]}
    blocks, links, _ = entry
    return {
        "blocks": [{"id": f"b{i}", "template": b["template"], "title": b["title"], "purpose": b["purpose"],
                    "targets": [{"name": k, "value": v} for k, v in b["targets"].items()]}
                   for i, b in enumerate(blocks, 1)],
        "links": [{"source": s, "target": t} for s, t in links],
        "uncovered": [],
    }


def answer(name: str, prompt: str, cases: list[dict[str, Any]]) -> str | None:
    """The reply to one pending exchange prompt (`name` is its file name), or None when these replies
    do not cover it."""
    kind = name.split("-")[0]
    case = next((c for c in cases if marker(c["prompt"]) in prompt), None)
    if case is None or case["id"] not in PLANS:
        return None
    entry = PLANS[case["id"]]
    if kind == "plan":
        if "Your previous plan:" in prompt and entry[0] != "unsupported":
            return None  # an in-scope plan was refused: answer its problems by hand
        # A re-plan of an out-of-scope request says no template builds what is uncovered: it stays uncovered.
        return json.dumps(plan_reply(entry), ensure_ascii=False)
    if entry[0] == "unsupported":
        return None
    if kind == "compose":
        if "Your previous attempt:" in prompt:
            return None  # a repair: answer what it shows by hand
        # Keep the template: the compose turn says to when it already does what the student asked, and every
        # request here is a template at targets inside its ranges.
        return json.dumps(USE_TEMPLATE)
    if kind == "narrate":
        return entry[2]
    return None


def fill(root: Path, cases: list[dict[str, Any]]) -> tuple[int, list[str]]:
    """Answer what is pending in an exchange directory; returns (answered, left for a person)."""
    answered, left = 0, []
    for pending in sorted((root / "pending").glob("*.txt")):
        reply = root / "replies" / pending.name
        if reply.exists():
            continue
        text = answer(pending.stem, pending.read_text(encoding="utf-8"), cases)
        if text is None:
            left.append(pending.stem)
            continue
        reply.write_text(text + "\n", encoding="utf-8", newline="\n")
        answered += 1
    return answered, left


if __name__ == "__main__":
    import sys

    import yaml

    if len(sys.argv) != 2:
        sys.exit("usage: python evals/authored/replies.py EXCHANGE_DIR")
    cases = yaml.safe_load((HERE.parent / "prompts.yaml").read_text(encoding="utf-8"))
    missing = {c["id"] for c in cases} - set(PLANS)
    if missing:
        sys.exit(f"no authored reply for {', '.join(sorted(missing))}")
    n, left = fill(Path(sys.argv[1]), cases)
    print(f"answered {n}; left for a reply written by hand: {len(left)} {' '.join(left)}")
