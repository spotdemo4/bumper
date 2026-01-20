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
read -r -d '' NIX_CONFIG <<EOF
extra-experimental-features = nix-command flakes
accept-flake-config = true
warn-dirty = false
always-allow-substitutes = true
fallback = true
EOF

export NIX_CONFIG