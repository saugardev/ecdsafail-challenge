# Phase bisect weirdness note

Two small targeted phase-bisect probes currently disagree in a surprising way:

- one probe that found a failing batch on the experimental full circuit and then
  compared generic vs experimental at the late cuts reported a divergence before
  `lam` free,
- a second probe that rebuilt the exact batch-10 sample from the experimental
  op stream showed matching phase masks at all late cuts.

## Likely interpretation
The disagreement itself is evidence that the phase bug is not a simple local
late-cut difference in the top-level prefix.

The most plausible remaining explanations are:
1. the divergence depends on *how* the batch is consumed together with the
   shared circuit-seeded RNG stream, not only on the raw point set,
2. or the phase defect is introduced after the cut we are probing, in logic not
   yet included in the probe circuit.

Either way, this reinforces the current hypothesis that the bug is a subtle
full-scaffold interaction rather than a one-line classical state mismatch in the
specialized step.
