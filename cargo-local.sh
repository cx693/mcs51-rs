#!/bin/sh
# Build helper for sandboxed/read-only CARGO_HOME setups.
#
# The ambient environment points CARGO_HOME and CARGO_TARGET_DIR at shared
# locations outside this workspace. This wrapper keeps both inside the tree so
# the toolchain can be exercised without writing anywhere else.
#
#   ./cargo-local.sh test --workspace
#
set -e
root=$(cd "$(dirname "$0")" && pwd)
CARGO_HOME="$root/.cargo-home"
CARGO_TARGET_DIR="$root/target"
export CARGO_HOME CARGO_TARGET_DIR
mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR"
exec cargo "$@"
