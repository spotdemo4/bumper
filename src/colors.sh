#!/usr/bin/env bash

# set TERM to linux in CI environments for tput compatibility
if [[ -n "${CI-}" || -z "${TERM-}" ]]; then
    TERM=linux
fi

# color support
if colors=$(tput -T "${TERM}" colors 2> /dev/null); then
    color_reset=$(tput -T "${TERM}" sgr0)
    color_bold=$(tput -T "${TERM}" bold)

    if [[ "$colors" -ge 256 ]]; then
        color_info=$(tput -T "${TERM}" setaf 189)
        color_warn=$(tput -T "${TERM}" setaf 216)
        color_success=$(tput -T "${TERM}" setaf 117)
    elif [[ "$colors" -ge 8 ]]; then
        color_warn=$(tput -T "${TERM}" setaf 3)
        color_success=$(tput -T "${TERM}" setaf 2)
    fi
fi

function bold {
    printf "%s%s%s\n" "${color_bold-}" "${1-}" "${color_reset-}"
}

function info {
    printf "%s%s%s\n" "${color_info-}" "${1-}" "${color_reset-}" >&2
}

function warn {
    printf "%s%s%s\n" "${color_warn-}" "${1-}" "${color_reset-}" >&2
}

function success {
    printf "%s%s%s\n" "${color_success-}" "${1-}" "${color_reset-}" >&2
}
