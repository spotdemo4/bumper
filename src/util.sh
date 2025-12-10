#!/usr/bin/env bash

function run {
    if [[ -n "${DEBUG-}" ]]; then
        "${@}" >&2
    elif [[ -n "${CI-}" ]]; then
        cmd "::group::${*}"
        "${@}" >&2
        cmd "::endgroup::"
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
