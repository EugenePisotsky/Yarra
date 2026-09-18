# Authored content

`world.project.sqlite` is the current editable world: painted ground/grass layers,
shared presets, trees and a curved cart road on an 8 m grid. The editor opens it by
default. The game reads only its published `assets/generated/world.runtime.sqlite`.

For a fresh checkout:

```bash
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
```

`init` creates a world only when the project is absent. Subsequent runs retain its
source and republish it. Normal **Save** writes source changes; **Save & Publish**
cooks and atomically replaces the runtime. The command-line equivalent is:

```bash
cargo run -p yarra-world-cook -- cook
```

The old `demo.project.sqlite` is retired from normal launches. Test fixtures require
explicit `create-demo` / `create-road-demo` commands and separate paths. The former
`demo` cooker and `sync-demo-vegetation` reset commands are removed. Vegetation is
edited through the editor; intentional catalog replacement uses `import-vegetation`.

Source and generated databases remain local and ignored by Git. Models live in
`source_assets`; placements reference `object_definitions`. Authored presets, layers,
coverage masks and road geometry are source records, not cooked render pages.
