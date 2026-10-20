# Working on Yarra

A Rust/Bevy 0.20 game and world editor. [README.md](README.md) says how to build and run it;
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) says which crate owns what.

## Code style

- Format with `cargo fmt --all`, then `cargo run -p yarra-tidy -- --write`. rustfmt adds no
  blank lines; the tidy tool adds one between items and around a function's multi-line steps.
  Beyond that, separate a function's steps with blank lines as you would paragraphs.
- Keep functions readable on one screen. Split a long system or UI function into named steps.
  Group system parameters into meaningful `SystemParam`s instead of allowing
  `too_many_arguments` where the group means something.
- Comments say why, not what, and describe the code as it is. History belongs in git: no
  "previously", "now", "v2", "legacy" or "removed" in names or comments.
- One implementation per job. Delete what is replaced instead of keeping it behind a flag, and
  extract a helper when a third copy of a pattern appears.
- No compatibility code. Change a format directly, bump its version and rebuild the world:
  no migrations, fallback readers or retired-flag messages. Reteya's inputs rebuild it
  (`content/reteya/README.md`).

## Checks

```sh
cargo fmt --all --check
cargo run -p yarra-tidy
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p yarra-shader-check
```

Rendering changes are judged in the running game against captures of the same views; see
[docs/WORKFLOWS.md](docs/WORKFLOWS.md).
