# Answer key (facilitator only)

For the facilitator, who does not need to know electronics, and for the reviewer reading
`answers.csv` afterwards. **Never show it to the student.** It says what a right answer to each task
contains, what a wrong one looks like, and what the screen should show, so you can tell at a glance
whether the tutor (or the student) is on track. You still never teach: "what would you ask the
tutor?" stays the only hint.

The values below are for the blocks as inserted with their defaults: a *Sine signal source* (1 V,
1 kHz) into a *Sallen-Key low-pass* with a 1 kHz cutoff. Its parts are R1 = 18 kΩ, R2 = 30 kΩ,
C1 = 10 nF (from the middle of the filter to the output) and C2 = 4.7 nF (to ground). Its checks read
**fc 996Hz ✓** (the cutoff: where the filter starts to cut the signal) and **Q 0.706 ✓** (how sharp
the bend is; 0.707 is the smoothest, "maximally flat").

## Task 1: why does the resistor have that value?

**A right answer says:** the four parts *together* set the cutoff, fc = 1/(2π·√(R1·R2·C1·C2)) ≈ 1 kHz,
and their ratios set Q ≈ 0.707. 18 kΩ and 30 kΩ are standard values that land close to that target with
these capacitors. A round 10 kΩ for R1 would move the cutoff up to about 1.34 kHz (996 Hz × √(18/10)).

**A wrong answer looks like:**
- One resistor and one capacitor alone set the cutoff: fc = 1/(2π·R1·C1) gives **884 Hz**. That is the
  formula for a simple RC filter, not this one.
- The resistor sets the gain or the amplification. This filter's gain is 1.
- Any part or value not on the screen.

## Task 2: Guide me: why is the output smaller at 1 kHz?

**The point to reach:** 1 kHz *is* the cutoff (996 Hz), and at the cutoff the output is about 0.707 of
the input, which is "−3 dB". With a 1 V input, the output is about **0.705 V (705 mV)**.

**A good tutor:** asks one guiding question (for example, "how does 1 kHz compare with the cutoff?").
When the student answers rightly, it says **yes** and stops asking. When they answer wrongly
(for example, "the op-amp loses signal"), it says *not quite* and points them back at the cutoff.

**Watch for:** the tutor asking the same question again after a right answer (the dry run's main
problem, fixed since), or calling a right answer "partly right".

## Task 3: move the cutoff to 500 Hz

**A right suggestion:** halve the cutoff by **doubling both resistors** (R1 → 36 kΩ, R2 → 60 kΩ) or
**doubling both capacitors** (C1 → 20 nF, C2 → 9.4 nF). Either predicts about **498 Hz** (996 Hz ÷ 2) and
keeps Q near 0.71.

**After Try it, the screen should show:**
- the card: **✓ Prediction held** (predicted about 498 Hz, measured about **499 Hz**);
- the filter's badge: **fc 499Hz (retuned)** in blue, not a red ✗. The block's target is still 1 kHz;
  the student moved it on purpose.

**Weak or wrong:** changing only one part. R1 alone to 36 kΩ gives about 724 Hz, not 500, and changes Q.
A prediction that misses by more than 10% shows "Prediction missed" on the card.

## Task 4: the student's own change

Open-ended. The explanation ("What changed?") should get the **direction** right:

| The student changes | The cutoff | The output at 1 kHz | Q (sharpness) |
| --- | --- | --- | --- |
| R1, R2, C1 or C2 **bigger** | goes **down** | gets smaller | C1 bigger: up (a peak); C2 bigger: down |
| R1, R2, C1 or C2 **smaller** | goes **up** | gets bigger (towards 1 V) | C1 smaller: down; C2 smaller: up |
| The source's **frequency** up (say 5 kHz) | **unchanged** (it belongs to the filter) | much smaller | unchanged |
| The source's **frequency** down (say 100 Hz) | unchanged | close to 1 V | unchanged |
| The source's **amplitude** (say 2 V) | unchanged | scales with it (about 1.4 V) | unchanged |
| The op-amp swapped for an LM358 | about the same | about the same | about the same |

**Wrong:** the direction reversed, or the cutoff "moving" because the source's frequency changed.

## Task 5: something broke

**The fault you introduce:** C1's pin 2 disconnected from the filter's output (README, "Each session").
Both checks fail; Q falls to about 0.1.

**A good tutor:** points at **C1**: one end of it is not connected to anything, so the output no longer
feeds back into the filter. For a beginner it says this in plain words, not "pin C1.2 is marked -".

**Fixed when:** C1's pin 2 is connected to the filter's output again (in the inspector, or with the
wire tool). The checks return to **fc 996Hz ✓, Q 0.706 ✓**.

## Judging any answer quickly

- **Numbers:** every number should be on screen (the inspector, the badges, the wires) or come with its
  arithmetic. A precise number with no source is a red flag.
- **Chips:** a part name in an answer is a clickable chip; plain text in square brackets means the tutor
  named something the circuit does not have.
- **Level:** a beginner answer with formulas first, long lists of voltages, or words like "ERC" and
  "operating point" is **weak**, even when it is right.
- **Mark it:** **ok**, **weak** (right but unhelpful) or **wrong** (a false claim, a wrong number or
  direction, an invented part). Note whether the student noticed a wrong one; that matters most.
