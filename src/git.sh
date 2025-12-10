#!/usr/bin/env bash

# https://github.com/gitleaks/gitleaks/issues/1364#issuecomment-2035545023
function git_check_safe() {
    local dir="$1"
    local safe_dirs=()

    if [[ "$(stat -c "%U" "${dir}")" == "$(whoami)" ]]; then
        return 0
    fi

    readarray -t safe_dirs < <(git config --global --get-all safe.directory)

    for safe_dir in "${safe_dirs[@]}"; do
        if [[ "$(realpath "${dir}")" == "$(realpath "${safe_dir}")" ]]; then
            return 0
        fi
    done

    warn "adding '${dir}' to git safe directories"
    git config --global --add safe.directory "${dir}"
}

# https://github.com/actions/checkout/issues/13
function git_check_user() {
    if [ -z "$(git config user.name)" ]; then
        warn "no user found, using default"
        git config --local user.name "github-actions[bot]"
        git config --local user.email "github-actions[bot]@users.noreply.github.com"
    fi

    info "git user: $(git config user.name) <$(git config user.email)>"
}

function get_impact() {
    local major_types="${1}"
    local minor_types="${2}"
    local patch_types="${3}"
    local skip_scopes="${4}"
    local last_hash="${5}"
    local last_version="${6}"
    local force="${7}"

    readarray -t major_types < <(array "${major_types:-"BREAKING CHANGE"}")
    readarray -t minor_types < <(array "${minor_types:-"feat"}")
    readarray -t patch_types < <(array "${patch_types:-"fix"}")
    readarray -t skip_scopes < <(array "${skip_scopes:-"ci"}")

    local impact=""
    local commits=()

    # check if we should force a version bump
    if [[ "${force}" == "true" ]]; then
        warn "forcing (at least) a PATCH version bump"
        impact="patch"
    fi

    # get semver impact from commits
    # https://www.conventionalcommits.org/en/v1.0.0/
    readarray -t commits < <(git log --pretty=format:"%s" "${last_hash}..HEAD")
    info "$(bold "checking ${#commits[@]} commits since last tag (${last_version})")"
    for commit in "${commits[@]}"; do
        local prefix
        local type
        local scope

        # skip commits that don't follow conventional commit format
        if [[ ! "${commit}" == *:* ]]; then
            info "skipped (convention): ${commit}"
            continue
        fi

        prefix=$(echo "${commit}" | cut -d ':' -f 1)
        type=$(echo "${prefix}" | cut -d '(' -f 1)
        scope=$(echo "${prefix}" | cut -s -d '(' -f 2 | cut -s -d ')' -f 1)

        # default empty scope to "none"
        if [[ -z "${scope}" ]]; then
            scope="none"
        fi

        # check if scope is in skip list
        for skip_scope in "${skip_scopes[@]}"; do
            if [[ "${scope,,}" == "${skip_scope,,}" ]]; then
                info "skipped (scope): ${commit}"
                continue 2
            fi
        done

        # if commit prefix ends with "!", it's a major change
        if [[ "${prefix: -1}" == "!" ]]; then
            info "$(bold "major:") ${commit}"
            impact="major"
            break
        fi

        # check for major, minor, patch types

        for major_type in "${major_types[@]}"; do
            if [[ "${type,,}" == "${major_type,,}" ]]; then
                info "$(bold "major:") ${commit}"
                impact="major"
                break 2
            fi
        done

        for minor_type in "${minor_types[@]}"; do
            if [[ "${type,,}" == "${minor_type,,}" ]]; then
                info "$(bold "minor:") ${commit}"
                impact="minor"
                continue 2
            fi
        done

        # skip checking for patches if already impactful
        if [[ -n "${impact}" ]]; then
            info "skipped (impact): ${commit}"
            continue
        fi

        for patch_type in "${patch_types[@]}"; do
            if [[ "${type,,}" == "${patch_type,,}" ]]; then
                info "$(bold "patch:") ${commit}"
                impact="patch"
                continue 2
            fi
        done

        info "skipped (type): ${commit}"
    done

    echo "${impact}"
}

function get_next_version() {
    local last_version="${1}"
    local impact="${2}"

    local major
    local minor
    local patch

    major=$(echo "${last_version}" | cut -s -d . -f 1)
    minor=$(echo "${last_version}" | cut -s -d . -f 2)
    patch=$(echo "${last_version}" | cut -s -d . -f 3)

    case "${impact}" in
        major) 
            major=$((major + 1))
            minor=0
            patch=0
            ;;
        minor) 
            minor=$((minor + 1))
            patch=0
            ;;
        patch)
            patch=$((patch + 1))
            ;;
    esac

    echo "${major}.${minor}.${patch}"
}

git_check_safe "$(pwd)"
git_check_user
