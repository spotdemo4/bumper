#!/usr/bin/env bash

# https://github.com/gitleaks/gitleaks/issues/1364
git config --global --add safe.directory "$(pwd)"

# ensure git user is set
if [ -z "$(git config user.name)" ]; then
    echo "no user found, using default"
    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"
fi
