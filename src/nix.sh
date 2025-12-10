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

    local packages_list
    packages_list=$(echo "${packages}" | jq -r --arg system "$system" '.packages[$system] | keys | join(", ")')
    echo "packages: ${packages_list}" >&2

    local packages_json
    packages_json=$(echo "${packages}" | jq -r --arg system "$system" '.packages[$system] | keys[]')

    echo "${packages_json}"
}
