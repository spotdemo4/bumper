#!/usr/bin/env bash

if [[ "${DOCKER-}" == "true" ]]; then
    # https://discourse.nixos.org/t/warning-about-home-ownership/52351
    chown -R "${USER}:${USER}" "${HOME}"
fi

NIX_ARGS=("--extra-experimental-features" "nix-command flakes" "--accept-flake-config" "--no-warn-dirty")

function nix_system () {
    local system

    system=$(nix "${NIX_ARGS[@]}" eval --impure --raw --expr "builtins.currentSystem")

    echo "${system}"
}

function nix_packages () {
    local system="$1"

    local packages
    packages=$(nix "${NIX_ARGS[@]}" flake show --json 2> /dev/null)

    local packages_json
    packages_json=$(echo "${packages}" | jq -r --arg system "${system}" '.packages[$system] | keys[]')

    echo "${packages_json}"
}
