#!/bin/sh
# Cargo's runner on macOS (.cargo/config.toml): the game runs from an app bundle beside its binary,
# `Yarra.app`, whose Info.plist (crates/app_game/Info.plist) names it a game. macOS gives Game Mode
# (CPU and GPU priority, the GPU held at its fast clocks under sustained load) only to an app it
# knows is a game; a bare binary is not one. The executable is a hard link, so the bundle costs
# nothing to refresh; `open target/release/Yarra.app` starts it as an app. Everything else (tests,
# the editor, tools) runs as it is.
binary="$1"
shift

if [ "$(basename "$binary")" != yarra-app-game ]; then
    exec "$binary" "$@"
fi

contents="$(dirname "$binary")/Yarra.app/Contents"
mkdir -p "$contents/MacOS"
ln -f "$binary" "$contents/MacOS/yarra-app-game"
cp "$(dirname "$0")/../crates/app_game/Info.plist" "$contents/Info.plist"
exec "$contents/MacOS/yarra-app-game" "$@"
