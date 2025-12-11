#!/usr/bin/env bash

# set TERM to linux in CI environments for tput compatibility
if [[ -n "${CI-}" || -z "${TERM-}" ]]; then
    TERM=linux
fi

# color support
if colors=$(tput -T "${TERM}" colors 2> /dev/null); then
    color_reset=$(tput -T "${TERM}" sgr0)
    color_bold=$(tput -T "${TERM}" bold)
    color_dim=$(tput -T "${TERM}" dim)

    if [[ "$colors" -ge 256 ]]; then
        color_info=$(tput -T "${TERM}" setaf 189)
        color_warn=$(tput -T "${TERM}" setaf 216)
        color_success=$(tput -T "${TERM}" setaf 117)
    elif [[ "$colors" -ge 8 ]]; then
        color_warn=$(tput -T "${TERM}" setaf 3)
        color_success=$(tput -T "${TERM}" setaf 2)
    fi
fi

function bold() {
    printf "%s%s%s\n" "${color_bold-}" "${1-}" "${color_reset-}"
}

function dim() {
    printf "%s%s%s\n" "${color_dim-}" "${1-}" "${color_reset-}"
}

function info() {
    printf "%s%s%s\n" "${color_info-}" "${1-}" "${color_reset-}" >&2
}

function warn() {
    printf "%s%s%s\n" "${color_warn-}" "${1-}" "${color_reset-}" >&2
}

function success() {
    printf "%s%s%s\n" "${color_success-}" "${1-}" "${color_reset-}" >&2
}

function run() {
    if [[ -n "${DEBUG-}" ]]; then
        "${@}" >&2
    elif [[ -n "${CI-}" ]]; then
        printf "%s%s%s%s\n" "::group::" "${color_success-}" "${*}" "${color_reset-}" >&2
        "${@}" >&2
        printf "%s\n" "::endgroup::" >&2
    elif tput cols &> /dev/null; then
        local width
        local line
        local clean
        local code

        width=$(tput cols)

        printf "\r\033[2K%s%s%s" "${color_success-}" "${*}" "${color_reset-}" >&2

        "${@}" 2>&1 | while IFS= read -r line; do
            clean=$(echo -e "${line}" | sed -e 's/\\n//g' -e 's/\\t//g' -e 's/\\r//g' | head -c $((width - 10)))
            printf "\r\033[2K%s%s%s" "${color_dim-}" "${clean}" "${color_reset-}" >&2
        done
        code=${PIPESTATUS[0]}

        printf "\r\033[2K" >&2

        return "${code}"
    else
        "${@}" &> /dev/null
    fi
}

function array() {
    local string="$1"
    local n_array=()
    local array=()

    # split by either spaces or newlines
    if [[ "${string}" == *$'\n'* ]]; then
        readarray -t n_array <<< "${string}"
    else
        IFS=" " read -r -a n_array <<< "${string}"
    fi

    # remove empty entries
    for item in "${n_array[@]}"; do
        if [[ -n "${item}" ]]; then
            array+=( "${item}" )
        fi
    done

    # return empty if no entries
    if [[ "${#array[@]}" -eq 0 ]]; then
        return
    fi

    printf "%s\n" "${array[@]}"
}
