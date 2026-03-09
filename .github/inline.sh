#!/usr/bin/env bash

regex="^source\s+\"(.+)\"$"

while IFS= read -r line; do
    if [[ "${line}" =~ ${regex} ]]; then
        file="${BASH_REMATCH[1]//\$DIR/"${PWD}/src"}"
        echo "# -- $(basename "${file}") --"
        tail -n +2 "${file}"
        echo
    else
      printf '%s\n' "${line}"
    fi
done < "$1"
