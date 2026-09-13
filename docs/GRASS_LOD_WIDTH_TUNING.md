# Smaller LOD widths — 2026-09-14

The requested width reduction is retained. The subsequent lighting experiments were removed.

## Width

Population retention remains 0.65. Its inverse still normalizes the density curve, but it no
longer directly sets width. `SPLIT_LOW_COVERAGE_WIDTH_SCALE` is now 1.30 instead of `1 / 0.65`.
Both the high-to-low morph and the prepared low shoulder use the same new value.

At the full density anchor, for a fully surviving root after the geometry transition:

| Width relative to authored maximum | Before | Trial |
| --- | ---: | ---: |
| Main shoulder | 2.05× | 1.73× |
| Companion base | 1.54× | 1.30× |

This reduces the density compensation by 15.5%. The main's 4/3 shoulder factor, curves, vertex
placement, root counts, LOD distances and density fades are unchanged. Further density reduction
still adds its existing compensation; the screen-space minimum-width rule can dominate for thin
or edge-on blades, so the reduction is not uniform over every distant pixel.
