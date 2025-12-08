#!/usr/bin/env bash

# https://github.com/gitleaks/gitleaks/issues/1364
git config --global --add safe.directory "$(pwd)"

function run {
    if [[ -n "${DEBUG}" ]]; then
        "$@"
    else
        "$@" &> /dev/null
    fi
}