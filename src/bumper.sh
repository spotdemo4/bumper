#!/usr/bin/env bash
# export PATH="${PATH}" placeholder, will be replaced in release

set -o errexit
set -o nounset
set -o pipefail
shopt -s extglob

# make source imports work
DIR="${BASH_SOURCE%/*}"
if [[ ! -d "$DIR" ]]; then DIR="$PWD"; fi

source "$DIR/util.sh"
source "$DIR/nix.sh"
source "$DIR/git.sh"
source "$DIR/bump.sh"

# get vars
readarray -t PATHS < <(array "${PATHS-}")
MAJOR_TYPES="${MAJOR_TYPES:-"BREAKING CHANGE"}"
MINOR_TYPES="${MINOR_TYPES:-"feat"}"
PATCH_TYPES="${PATCH_TYPES:-"fix"}"
SKIP_SCOPES="${SKIP_SCOPES:-"ci"}"
COMMIT="${COMMIT:-true}"
PUSH="${PUSH:-true}"
FORCE="${FORCE:-false}"
ALLOW_DIRTY="${ALLOW_DIRTY:-false}"

# get args
if [[ "$#" -gt 0 ]]; then
    PATHS+=( "${@}" )
fi

# use current dir if no paths provided or in CI
if [[ "${#PATHS[@]}" -eq 0 || -n "${CI-}" ]]; then
    PATHS+=( "$(pwd)" )
fi

# validate the git environment is set up correctly
if [[ "${ALLOW_DIRTY}" != "true" ]] && (! git diff --staged --quiet || ! git diff --quiet); then
    warn "please commit or stash changes before running bumper"
    exit 1
fi
if ! git fetch --all --tags --quiet; then
    warn "could not fetch commits and tags"
    exit 1
fi
if ! branch=$(git rev-parse --abbrev-ref HEAD 2> /dev/null); then
    warn "not on a branch"
    exit 1
fi
if ! last_hash=$(git rev-list --tags --max-count=1 2> /dev/null); then
    warn "no git tags found, please create a tag first"
    exit 1
fi
if ! last_version=$(git describe --tags "${last_hash}" 2> /dev/null); then
    warn "no git tags found, please create a tag first"
    exit 1
fi

# strip leading 'v' from version
last_version=${last_version#v}

# determine impact
info
impact=$(get_impact "${MAJOR_TYPES}" "${MINOR_TYPES}" "${PATCH_TYPES}" "${SKIP_SCOPES}" "${last_hash}" "${last_version}" "${FORCE}")
if [[ -z "${impact}" ]]; then
    success "no new impactful commits since last tag (v${last_version})"
    exit 0
fi
info "$(bold "impact: ${impact}")"
info

# get next version
next_version=$(get_next_version "${last_version}" "${impact}")
info "$(bold "v${last_version} -> v${next_version}")"
info

# perform bumps
for bump_path in "${PATHS[@]}"; do
    if [[ -f "${bump_path}" ]]; then
        bump_file "${bump_path}" "${last_version}" "${next_version}"
    elif [[ -d "${bump_path}" ]]; then
        bump_dir "${bump_path}" "${next_version}"
    else
        warn "file or directory not found: ${bump_path}"
    fi
done
info

# check for staged changes
if git diff --staged --quiet; then
    warn "$(bold "no changes to commit")"
    exit 1
fi

# commit
if [[ "${COMMIT}" == "false" ]]; then
    success "skipping commit, tag and push"
    exit 0
fi
info "committing: v${last_version} -> v${next_version}"
run git commit -m "bump: v${last_version} -> v${next_version}"
run git tag -a "v${next_version}" -m "bump: v${last_version} -> v${next_version}"

# push
if [[ "${PUSH}" == "false" ]]; then
    success "skipping push"
    exit 0
fi
success "pushing changes to ${branch}"
run git push --atomic origin "${branch}" "v${next_version}"
