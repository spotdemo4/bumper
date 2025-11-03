#!/usr/bin/env bash

cd "${GITHUB_ACTION_PATH}"

if ! ./bumper.sh; then
    if [[ "${?}" -eq 1 ]]; then
        exit 1
    fi

    echo "missing dependency, falling back to binary"
    wget "https://github.com/spotdemo4/bumper/releases/download/v0.1.20/bumper-x86_64-linux" -O bumper
    chmod +x bumper
    bumper
fi