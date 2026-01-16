#!/usr/bin/env bash

function nix_system () {
    local system

    system=$(nix "${NIX_ARGS[@]}" eval --impure --raw --expr "builtins.currentSystem")

    echo "${system}"
}

function nix_packages () {
    local system="$1"

    local packages
    packages=$(nix "${NIX_ARGS[@]}" flake show --json 2> /dev/null)

    if [[ "$(echo "${packages}" | jq 'has("packages")')" == "false" ]]; then
        warn "flake has no packages"
        echo ""
        return
    fi

    local packages_json
    packages_json=$(echo "${packages}" | jq -r --arg system "${system}" '.packages[$system] | keys[]')

    echo "${packages_json}"
}

NIX_ARGS=("--extra-experimental-features" "nix-command flakes" "--accept-flake-config" "--no-warn-dirty")

# https://discourse.nixos.org/t/warning-about-home-ownership/52351
if [[ "${DOCKER-}" == "true" && -n "${CI-}" ]]; then
    chown -R "${USER}:${USER}" "${HOME}"
fi
