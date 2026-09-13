# Retired grass shadow experiments

**Both implementations were rejected by the user and removed on 2026-09-09.** The raised sheet
produced a blurry dark pattern and a moving zoom boundary. The replacement clump-mask caster also
failed, particularly from above. Neither is an accepted baseline or a basis for further tuning.
The current direction is [Grass shadow restart](GRASS_SHADOW_RESTART.md).

The renderer modules, shader, caster-specific tests, game flag, audit switch, editor controls and
caster-only scene metadata have been removed. Existing grass rendering, root color/AO and ordinary
object-shadow reception are retained. The zoom, overhead and traversal reproduction tools are kept;
a vertical top-down route is also available. There is no active grass shadow caster.

## Evidence retained

- `tmp/grass-shadow-2026-09-09/`: raised-sheet captures, four-run timing comparison and build identity.
- `tmp/grass-shadow-review-2026-09-09/`: bias/noise diagnostics, clump-mask captures, tests and timings.
- `tmp/grass-shadow-restart-2026-09-09/rejected-implementation/`: source files, integration patch and
  the previous full experiment report. This is an archive, not compiled runtime code.

The sheet comparison reported 4.220 ms off / 4.199 ms on. The clump-mask comparison reported
4.053 ms off / 4.018 ms on. Both differences were within run variation. These measurements establish
neither visual success nor zero added work, and do not establish phone or sustained thermal viability.
The clump version allocated 492,200 bytes and kept 512 triangles per source page, but increased
vertices per page from 289 to 1024. Low counts and bounded resources did not make its result useful.

Bias compensation had a valid narrow regression: with 10× receiver bias, aggregate shadow darkening
changed by less than 1%; removing compensation erased the shadow. That test verified an integration
property, not resemblance to grass. The entire caster and its test have been retired together.

The previous near/far captures and passing tests must not be cited as acceptance of the replacement.
The user's top-down rejection overrides that assessment. Future work must validate spatial
correspondence and view consistency before advancing to full-field integration and benchmarking.
