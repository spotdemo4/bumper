#!/usr/bin/env bash

# ensure git user is set
if [ -z "$(git config user.name)" ]; then
    echo "no user found, using default"
    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"
fi

echo "git user: $(git config user.name) <$(git config user.email)>"

# download deps with nix if available
if command -v nix &> /dev/null && ! command -v nix-update &> /dev/null; then
    echo "::group::nix-update not found, installing via nix"
    UPDATE_PATH=$(
        nix shell nixpkgs#nix-update \
            --inputs-from . \
            --command bash \
            -c "which nix-update"
    )
    dirname "${UPDATE_PATH}" >> "${GITHUB_PATH}"
    echo "::endgroup::"
fi

# run bumper, fall back to binary if deps are missing
if ! "${GITHUB_ACTION_PATH}/bumper.sh"; then
    if [[ ! "${?}" -eq 2 ]]; then
        exit 1
    fi

    echo "::group::missing dependency, falling back to binary"
    wget "https://github.com/spotdemo4/bumper/releases/download/v0.1.20/bumper-x86_64-linux" -O "${GITHUB_ACTION_PATH}/bumper"
    chmod +x "${GITHUB_ACTION_PATH}/bumper"
    echo "::endgroup::"

    "${GITHUB_ACTION_PATH}/bumper"
fi