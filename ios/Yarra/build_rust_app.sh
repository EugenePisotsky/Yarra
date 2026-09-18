#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CARGO_EXECUTABLE="${CARGO_EXECUTABLE:-${CARGO_HOME:-$HOME/.cargo}/bin/cargo}"

if [[ ! -x "$CARGO_EXECUTABLE" ]]; then
    echo "Cargo was not found at $CARGO_EXECUTABLE" >&2
    echo "Set CARGO_EXECUTABLE in the Xcode build environment if Rust is installed elsewhere." >&2
    exit 2
fi

export PATH="$(dirname "$CARGO_EXECUTABLE"):/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export CARGO_TARGET_DIR="$DERIVED_FILE_DIR/cargo"

case "$PLATFORM_NAME" in
    iphoneos)
        RUST_TARGET="aarch64-apple-ios"
        ;;
    iphonesimulator)
        case "$CURRENT_ARCH" in
            arm64)
                RUST_TARGET="aarch64-apple-ios-sim"
                ;;
            x86_64)
                RUST_TARGET="x86_64-apple-ios"
                ;;
            *)
                echo "Unsupported iOS simulator architecture: $CURRENT_ARCH" >&2
                exit 2
                ;;
        esac
        ;;
    *)
        echo "Unsupported Apple platform: $PLATFORM_NAME" >&2
        exit 2
        ;;
esac

PROFILE="debug"
CARGO_ARGUMENTS=(
    build
    --manifest-path "$REPOSITORY_ROOT/Cargo.toml"
    --locked
    --package yarra-app-game
    --target "$RUST_TARGET"
)
if [[ "$CONFIGURATION" != "Debug" ]]; then
    PROFILE="release"
    CARGO_ARGUMENTS+=(--release)
fi

RUNTIME_DATABASE="$REPOSITORY_ROOT/assets/generated/world.runtime.sqlite"
"$CARGO_EXECUTABLE" run \
    --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
    --locked \
    --package yarra-world-cook \
    -- \
    cook \
    "$REPOSITORY_ROOT/content/world.project.sqlite" \
    "$RUNTIME_DATABASE"

"$CARGO_EXECUTABLE" "${CARGO_ARGUMENTS[@]}"

EXECUTABLE_SOURCE="$CARGO_TARGET_DIR/$RUST_TARGET/$PROFILE/yarra-app-game"
EXECUTABLE_DESTINATION="$TARGET_BUILD_DIR/$EXECUTABLE_PATH"
/bin/mkdir -p "$(dirname "$EXECUTABLE_DESTINATION")"
/usr/bin/install -m 755 "$EXECUTABLE_SOURCE" "$EXECUTABLE_DESTINATION"

ASSET_DESTINATION="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/assets"
/bin/mkdir -p \
    "$ASSET_DESTINATION/generated" \
    "$ASSET_DESTINATION/shaders" \
    "$ASSET_DESTINATION/packs/terrain" \
    "$ASSET_DESTINATION/local/characters/female_main" \
    "$ASSET_DESTINATION/local/forest_tree_starter_kit/runtime/tree_07" \
    "$ASSET_DESTINATION/local/terrain/temperate_meadow/runtime"
/usr/bin/install -m 644 "$RUNTIME_DATABASE" "$ASSET_DESTINATION/generated/world.runtime.sqlite"
/usr/bin/ditto "$REPOSITORY_ROOT/assets/shaders" "$ASSET_DESTINATION/shaders"
/usr/bin/install -m 644 "$REPOSITORY_ROOT/assets/packs/terrain/prepared.terrain-prepared" "$ASSET_DESTINATION/packs/terrain/prepared.terrain-prepared"
/usr/bin/install -m 644 \
    "$REPOSITORY_ROOT/assets/local/characters/female_main/female_main_locomotion.glb" \
    "$ASSET_DESTINATION/local/characters/female_main/female_main_locomotion.glb"
/usr/bin/ditto \
    "$REPOSITORY_ROOT/assets/local/forest_tree_starter_kit/runtime/tree_07/summer" \
    "$ASSET_DESTINATION/local/forest_tree_starter_kit/runtime/tree_07/summer"
/usr/bin/ditto \
    "$REPOSITORY_ROOT/assets/local/terrain/temperate_meadow/runtime" \
    "$ASSET_DESTINATION/local/terrain/temperate_meadow/runtime"
