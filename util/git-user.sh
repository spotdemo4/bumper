#!/usr/bin/env bash

if [ -z "$(git config user.name)" ]; then
    echo "no user found, using default"
    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"
fi

echo "git user: $(git config user.name) <$(git config user.email)>"