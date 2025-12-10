#!/usr/bin/env bash

# https://github.com/gitleaks/gitleaks/issues/1364#issuecomment-2035545023
function git_check_safe() {
    local dir="$1"
    local safe_dirs=()

    if [[ "$(stat -c "%U" "${dir}")" == "$(whoami)" ]]; then
        return 0
    fi

    readarray -t safe_dirs < <(git config --global --get-all safe.directory)

    for safe_dir in "${safe_dirs[@]}"; do
        if [[ "$(realpath "${dir}")" == "$(realpath "${safe_dir}")" ]]; then
            return 0
        fi
    done

    echo "adding '${dir}' to git safe directories"
    git config --global --add safe.directory "${dir}"
}

# https://github.com/actions/checkout/issues/13
function get_check_user() {
    if [ -z "$(git config user.name)" ]; then
        echo "no user found, using default"
        git config --global user.name "github-actions[bot]"
        git config --global user.email "github-actions[bot]@users.noreply.github.com"
    fi

    echo "git user: $(git config user.name) <$(git config user.email)>"
}

git_check_safe "$(pwd)"
get_check_user
