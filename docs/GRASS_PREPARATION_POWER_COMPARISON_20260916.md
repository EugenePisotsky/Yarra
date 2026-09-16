# Matched 96-root preparation comparison — September 16, 2026

GP-020 completes the original-buffer control for GP-019. **The enlarged buffer
reduces GPU duration, but this matched pair shows no energy or smoothness win.**
Keep the production capacity at 131,072 blades and the 524,288 candidate opt-in.
Both configurations mostly maintain 120 fps, with recurring brief late stalls.
The next priority is explaining those stalls, rather than increasing density.

**Subsequent user steering:** the priority is now reducing heat in the first few
minutes. The timing trace proposed below is deferred. A short capture at the
known late event still needs roughly 12 minutes of game runtime, and the user
requested no long runs. [GP-021](GRASS_EARLY_HEAT_20260916.md) records the early
power review and next code candidate; the original measured comparison remains
unchanged. No trace or new game run was launched.

## Matched workload

Enlarged session `20260916-212946`; original/default session `20260916-220712`.
Binary, database, canopy and shader hashes match. The sole requested setting
difference is `prepared_blades`: 524,288 versus 131,072. Audited allocations are
70,516,748 versus 20,185,100 bytes (+48 MiB for enlarged). No rebuild was involved.

M2 Max, 38 GPU cores, 32 GiB; fullscreen 3456×2168 surface / 2592×1626 world,
96 authored roots/m², Balanced, 4× MSAA, low-walk, wind on, repro prepass off,
120 fps cap, counters off. Each run has 30 seconds app-closed idle, 60 seconds
warmup and 900 seconds measured. Both retain focus and constant source/prepared
terrain residency. Default-run audits confirm AutoVsync and Low Power Mode off.
Numeric density still uses the existing adaptive near-detail reach (~4.79 m).

This is one chronological enlarged/default pair, not a counterbalanced repeat.
AC power was recorded in both; battery was 100% charged for enlarged and 98%
charging for default. App-closed idle CPU+GPU estimates were 0.80 W and 0.06 W.
Initial temperature and fan RPM were not measured. These differences limit
precise causal energy attribution; they do not invalidate the workload match or
justify attributing the outcome to background applications.

## Measured comparison

| Metric | Original 131,072 | Enlarged 524,288 |
| --- | ---: | ---: |
| Whole-run average app fps | 119.557 | 119.600 |
| Whole-run GPU power | 19.68 W | 20.52 W |
| Whole-run CPU+GPU power | 24.06 W | 25.13 W |
| Final five minutes: app fps | 119.394 | 119.266 |
| Final five minutes: GPU power | 19.48 W | 21.01 W |
| Final five minutes: CPU+GPU power | 23.84 W | 25.38 W |
| Final five minutes: GPU active MHz | 1,378 | 1,340 |
| Final five minutes: GPU active residency | 96.13% | 87.50% |
| Final five minutes: HUD GPU mean | 6.44 ms | 4.90 ms |
| Whole-run late app intervals | 399 / 107,602 (0.371%) | 366 / 107,640 (0.340%) |
| Worst approximately one-second app window | 73.00 fps | 77.31 fps |
| Full app sample windows below 95% of cap | 14 | 11 |

The enlarged buffer's final-five-minute GPU power is **1.53 W / 7.85% higher**;
combined CPU+GPU is **1.54 W / 6.47% higher**. Whole-run differences are smaller
(+0.83 W GPU, +1.07 W combined) because thermal transients differ. There is no
measured power saving to claim from this pair. Repeating in reversed order would
be necessary to establish a reproducible regression of this size.

The GPU-duration improvement remains real evidence alongside GP-018's reduced
vertex instructions, although these live HUD times were measured at different
clocks. The candidate also uses more memory and increases storage traffic in
matched replay. That is a plausible energy tradeoff, not proof of the cause of
the extra watts. GPU active residency measures time active, not the fraction of
all execution resources consumed; neither 87.5% nor 96.1% yields a linear density
allowance. Lower duration or residency does not automatically mean lower power.

Watts are CPU/GPU subsystem estimates, not isolated grass/game watts or total
laptop consumption. App updates and overlapping HUD samples are separate
measurements. Whole-run HUD interval p95 is 8.33 ms for both and hides the short
stalls; the worst app window is not a 1% low or instantaneous display rate.

## Default-buffer timeline

Times start at measurement, after the one-minute warmup. Whole sample intervals
are used inside each bin; power alignment is approximately one second.

| Measured period | App fps | GPU W | CPU+GPU W | Active GPU MHz | Late updates |
| --- | ---: | ---: | ---: | ---: | ---: |
| First five minutes | 119.336 | 20.03 | 24.45 | 1,310 | 199 |
| Middle five minutes | 119.957 | 19.55 | 23.92 | 1,351 | 13 |
| Final five minutes | 119.394 | 19.48 | 23.84 | 1,378 | 181 |

Six late updates fall in sample windows crossing five-minute boundaries and are
excluded from those bins, but included in the whole-run total. Power coverage is
898.84 of 900 seconds. The maximum app update interval is 17.97 ms.

- **3:46:** pressure becomes moderate; **3:48–6:04:** heavy; then moderate until
  **7:26**, followed by nominal through the end. The app's fair → nominal
  transition agrees within the audit/sample cadence.
- The early dip repeats at approximately the five-second camera-route period
  while GPU residency is around 99–100%; the lowest sampled clock is 1,057 MHz.
  Around 4:15–4:50, the weaker app windows are approximately 108–114 fps.
- Later nominal-pressure events appear around **11:43–11:44**, **12:47** and
  **13:52**, with lowest one-second rates **91.94**, **107.09** and **73.00 fps**.
  The separations between those sample endpoints are **64.28** and **64.32 s**.
  A smaller dip near 11:15 is also retained in the data; not every dip belongs
  to the apparent 64-second pattern.
- The enlarged run had corresponding late events near 11:49, 12:53 and 13:57.
  This repeated pattern across both capacities makes an enlarged-buffer-specific
  failure less likely. It does not prove a single cause or exact periodicity.

Metal HUD packets confirm 16.67 ms presentation bursts at all three late events
in the default run. Near 11:43 and 13:52, sampled GPU clocks/power drop briefly.
The weaker **12:47** event is different: nearby GPU clocks remain about 1,373 MHz
and HUD GPU duration about 6.3 ms while presentation intervals lengthen. This
supports examining submission/event-loop/presentation timing as well as GPU work;
it does not by itself identify the CPU, limiter, compositor or driver as the cause.
Nominal thermal labels also do not exclude every low-level power/thermal response.

User observation, preserved verbatim:

> in the end it was mostly the same. fans I think started working faster but not sure. gpu time was also larger I think

Record the end-of-run heat/fan impression as mostly the same, the faster-fan
timing as uncertain. The larger GPU duration is confirmed by the HUD comparison.
There is no measured fan/temperature series to establish a noise or heat winner.

## Decision and next diagnostic

1. **Do not promote the enlarged arena for the heat/power goal.** Keep the default
   unchanged. Retain the option and replay evidence for a possible throughput
   tradeoff; do not relabel it a failed work-reduction experiment.
2. **Investigate recurring late stalls before another density increase.** Both
   capacities show them, and one default-buffer event lacks an obvious GPU-duration
   overrun. The current profiler uses an 8.333 ms reactive event-loop timer together
   with AutoVsync; inspect this timing interaction as a hypothesis, not a diagnosis.
3. **Next useful capture:** a bounded native CPU/GPU/presentation trace around
   measured seconds 695–715 (game elapsed roughly 755–775), which brackets the
   first larger late event in both runs. Preserve the same frozen workload, keep
   counters/replay performance overrides off, log actual trace start/end and
   focus, and treat the trace as diagnostic rather than another energy acceptance
   run. Attaching a tracer can perturb or suppress the event; verify it appears
   in the captured app/HUD data before attributing it. No trace was launched in
   this follow-up, and no additional long power run is needed just to confirm
   that the default buffer also stalls.

The denser field is mostly near 120 on this Mac, but neither run establishes
consistently smooth 120 with spare capacity for a populated game. Native 1440p60
on named mainstream PC GPUs remains necessary for a shipping density budget.

[Preserved evidence](performance/20260916-220712/README.md) includes original
raw logs/report, 10/30-second and five-minute analysis, the matched comparison,
both user observations and all input hashes. Original report and derived analyses
were reproduced exactly offline; the archive is checksummed. No renderer,
production settings or profiling tool behavior changed in this follow-up.
