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

RUNTIME_DATABASE="$REPOSITORY_ROOT/assets/generated/demo.runtime.sqlite"
"$CARGO_EXECUTABLE" run \
    --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
    --locked \
    --package yarra-world-cook \
    -- \
    demo \
    "$REPOSITORY_ROOT/content/demo.project.sqlite" \
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
    "$ASSET_DESTINATION/local/forest_tree_starter_kit/runtime/tree_07"
/usr/bin/install -m 644 "$RUNTIME_DATABASE" "$ASSET_DESTINATION/generated/demo.runtime.sqlite"
/usr/bin/ditto "$REPOSITORY_ROOT/assets/shaders" "$ASSET_DESTINATION/shaders"
/usr/bin/ditto \
    "$REPOSITORY_ROOT/assets/local/forest_tree_starter_kit/runtime/tree_07/summer" \
    "$ASSET_DESTINATION/local/forest_tree_starter_kit/runtime/tree_07/summer"
