#!/usr/bin/env bash

function run {
    if [[ -n "${DEBUG}" ]]; then
        "$@"
    else
        "$@" &> /dev/null
    fi
}