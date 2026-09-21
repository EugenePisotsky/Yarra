# Temporal configuration options, 2026-09-21

The public `MTLFXTemporalScalerDescriptor`/`MTLFXTemporalScalerBase` APIs in the
installed SDK expose no model-quality, iteration-count or history-length control.
Dimensions and texture formats can change cost. Exposure, history reset, motion,
jitter and reactive-mask settings serve different purposes and should not be
presented as interchangeable performance presets.

| Configuration | Current state and evidence |
| --- | --- |
| Input/output dimensions | Input scaling is exposed in the game. Lowering the reconstruction output also reduces its pixel workload, with a resolution tradeoff. Earlier paired-size native checks measured about 2.37 ms at 1728×971 → 3456×1942 and 1.44 ms at 1280×720 → 2560×1440; both input and output changed. |
| Synchronous initialization | Already enabled. Compiles the fast implementation before use; it does not select a different steady-state quality level. |
| Auto exposure | Enabled. Earlier compact-input synthetic tests measured about 2.33–2.35 ms enabled and 2.36 ms disabled. No useful gain was demonstrated. |
| Compact input allocation / fixed resolution | Previously tested; roughly 0.03 ms difference. Not a demonstrated explanation for the large cost. |
| Color/output formats | Current renderer uses RGBA16Float. Packed RG11B10Float is accepted and completes native reconstruction on this Mac; comparison below. |
| Reactive mask | Disabled. A quality tool for controlling history contribution, not an exposed lighter reconstruction model. |
| Jitter, motion scale, reversed depth, reset | Correctness/history inputs. Steady sampled frames were not resetting history. |

See Apple's [descriptor](https://developer.apple.com/documentation/metalfx/mtlfxtemporalscalerdescriptor),
[initialization behavior](https://developer.apple.com/documentation/metalfx/mtlfxtemporalscalerdescriptor/requiressynchronousinitialization),
and [packed HDR format](https://developer.apple.com/documentation/metal/mtlpixelformat/rg11b10float).

## Packed HDR: promising timing, unproven game benefit

Native-only A/B/A, same executable, 1728×971 input content in 3456×1942 allocations,
3456×1942 output, automatic exposure, constant empty input, 120 calls/s, 8-second
warmup and 8-second measurement. Only the color and output formats change. Depth
and motion remain Depth32Float and RG16Float. All three calls completed successfully,
with 961 measured reconstruction frames each.

| Format | Native command-buffer median | Mean GPU clock | Estimated GPU power |
| --- | ---: | ---: | ---: |
| RGBA16Float, before | 4.835 ms | 668 MHz | 2.16 W |
| RG11B10Float, input and output | 3.494 ms | 753 MHz | 5.02 W |
| RGBA16Float, repeat | 4.842 ms | 669 MHz | 2.14 W |

The baseline repeated closely, and packed HDR had a lower measured duration.
**This is not a proven fixed-clock speedup or a power/thermal improvement.** GPU
clocks and system-wide estimated GPU power increased in the packed run; telemetry
has seven measurement-window samples per case. Native command-buffer spans are
not interchangeable with whole-game times. No sustained run was performed.

Packed HDR uses 32 bits/pixel versus RGBA16Float's 64 for these input/output images.
It does not halve MetalFX's internal storage or all renderer bandwidth. It has less
precision, no alpha and no negative color values. These tests used a constant
positive color and did not assess real-scene visual quality. Descriptor creation
also accepted input-only and output-only packed formats, but those combinations
were not benchmarked.

No production formats were changed. A useful follow-up must check complete game
frame time, heat/power and temporal image quality before adopting this format.
[Results, sources and executable hash](config-options/results.json) and raw logs
are retained with the bounded native runner.
