# Pair1 single-halve finding

On the actual first strict failing batch for `k = 4` (batch 10):

- phase before the pair1 halve chain: `0x0000040000000000`
- phase after **one** halve operation: `0x0000040000000000`

So the cancellation observed after the full `pair1_halve` chain does **not**
happen immediately. It is an accumulated effect over many halve steps.

## Interpretation
This rules out the idea that there is a simple special correction in the very
first halve application. The phase behavior of `pair1_halve` is a long-range
cancellation phenomenon across the full chain.

That keeps the focus on the interface between:
- the specialized inverse prefix,
- the repeated halve chain as a whole,
- and `pair1_mul2`,

rather than any one obvious single halve gate.
