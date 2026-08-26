#!/usr/bin/env bash

# SPDX-License-Identifier: MIT OR Apache-2.0

# Check that features are additive and compare the public API with a baseline.
#
# Usage: check-semver.sh [BASELINE_COMMIT]
#
# Exit codes:
#   0 - No breaking changes.
#   1 - Non-additive features or a breaking change after version 1.0.0.
#   2 - A breaking change before version 1.0.0.

set -euo pipefail

BASELINE_COMMIT="${1:-$(git rev-parse master)}"
PACKAGE_ROOT="$(git rev-parse --show-toplevel)"
BASELINE_DIR=""

command -v cargo-semver-checks >/dev/null 2>&1 || {
    echo "cargo-semver-checks was not found on \$PATH"
    exit 1
}
command -v jq >/dev/null 2>&1 || {
    echo "jq was not found on \$PATH"
    exit 1
}

PACKAGE_METADATA="$(cargo metadata --format-version 1 --no-deps)"
PACKAGE_NAME="$(jq -r '.packages[0].name' <<< "$PACKAGE_METADATA")"
PACKAGE_MAJOR="$(jq -r '.packages[0].version | split(".")[0] | tonumber' <<< "$PACKAGE_METADATA")"

# The API checks compile every feature, but they do not need bundled binaries.
export DOCS_RS=1

# Remove the temporary baseline worktree, if it was created.
cleanup() {
    if [ -n "$BASELINE_DIR" ]; then
        git -C "$PACKAGE_ROOT" worktree remove --force "$BASELINE_DIR" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# Check that enabling features does not remove public API items.
check_feature_additivity() {
    echo "Checking that cargo features are additive..."

    local -a current_features=()
    while IFS= read -r feature; do
        current_features+=("--current-features" "$feature")
    done < <(jq -r '.packages[0].features | keys[] | select(. != "default")' <<< "$PACKAGE_METADATA")

    cargo semver-checks --quiet \
        -p "$PACKAGE_NAME" \
        --release-type minor \
        --only-explicit-features \
        --baseline-root "$PACKAGE_ROOT" \
        --baseline-features "" \
        "${current_features[@]}"
}

# Compare the public API with the selected baseline commit.
check_baseline() {
    echo "Checking public API against $BASELINE_COMMIT..."

    BASELINE_DIR=$(mktemp -d)
    git -C "$PACKAGE_ROOT" worktree add --detach "$BASELINE_DIR" "$BASELINE_COMMIT"

    local breaking=false
    local -a feature_args
    for variant in all-features no-default-features; do
        echo "Checking $PACKAGE_NAME ($variant)..."

        case "$variant" in
            all-features)
                feature_args=("--all-features")
                ;;
            no-default-features)
                feature_args=("--only-explicit-features")
                ;;
        esac

        if ! cargo semver-checks --quiet \
            -p "$PACKAGE_NAME" \
            --release-type minor \
            --baseline-root "$BASELINE_DIR" \
            "${feature_args[@]}"; then
            breaking=true
        fi
    done

    if [ "$breaking" = false ]; then
        return 0
    fi

    if [ "$PACKAGE_MAJOR" -ge 1 ]; then
        return 1
    fi

    return 2
}

check_feature_additivity && check_baseline
