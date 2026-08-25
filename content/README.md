# Authored content

`demo.project.sqlite` is the writable authoring representation for the initial
world. The game never opens it. `yarra-world-cook` reads a committed snapshot and
publishes an independently validated, immutable runtime database under
`assets/generated/`.

This first database stores an explicit default world space, a large overworld,
a separate interior, flat cell appearance, and stable tree placements. The
schema separates sparse placements from cooked terrain/object pages so future
editor operations do not become runtime loading operations.

Models live in `source_assets`; placements reference `object_definitions`.
Definitions currently contain only identity, display/visual references, and a
render-only or proximity activation policy. Gameplay capabilities and their
typed attribute tables will be added when the first real interactive object is
implemented.
