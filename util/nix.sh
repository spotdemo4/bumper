#!/usr/bin/env bash

if command -v nix &> /dev/null && ! command -v nix-update &> /dev/null; then
    echo "nix-update not found, installing via nix"

    UPDATE_PATH=$(
        nix shell nixpkgs#nix-update \
            --inputs-from . \
            --command bash \
            -c "which nix-update"
    )
    dirname "${UPDATE_PATH}" >> "${GITHUB_PATH}"
fi