#!/usr/bin/env bash

function nix_system () {
    local system

    system=$(nix eval --impure --raw --expr "builtins.currentSystem" 2> /dev/null)

    echo "${system}"
}

function nix_packages () {
    local system="$1"

    local packages
    packages=$(nix flake show --json 2> /dev/null)

    if [[ "$(echo "${packages}" | jq 'has("packages")')" == "false" ]]; then
        warn "flake has no packages"
        return
    fi

    local packages_json
    packages_json=$(echo "${packages}" | jq -r --arg system "${system}" '.packages[$system] | keys[]')

    echo "${packages_json}"
}

# https://discourse.nixos.org/t/warning-about-home-ownership/52351
if [[ "${DOCKER-}" == "true" && -n "${CI-}" ]]; then
    chown -R "${USER}:${USER}" "${HOME}"
fi

# https://nix.dev/manual/nix/latest/command-ref/conf-file
NIX_CONFIG="extra-experimental-features = nix-command flakes"$'\n'
NIX_CONFIG+="accept-flake-config = true"$'\n'
NIX_CONFIG+="warn-dirty = false"$'\n'
NIX_CONFIG+="always-allow-substitutes = true"$'\n'
NIX_CONFIG+="fallback = true"$'\n'

if [[ -n "${GITHUB_TOKEN-}" ]]; then
    NIX_CONFIG+="access-tokens = github.com=${GITHUB_TOKEN}"$'\n'
fi

export NIX_CONFIG
