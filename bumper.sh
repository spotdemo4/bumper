#!/usr/bin/env bash

# color support
reset_color=""
bold_color=""
info_color=""
dialog_color=""
warn_color=""
success_color=""

if colors=$(tput colors 2> /dev/null); then
    reset_color=$(tput sgr0)
    bold_color=$(tput bold)

    if [[ "$colors" -ge 256 ]]; then
        info_color=$(tput setaf 189)
        dialog_color=$(tput setaf 246)
        warn_color=$(tput setaf 216)
        success_color=$(tput setaf 117)
    fi
fi

function bold {
    printf "\n%s%s%s\n" "${bold_color}" "$1" "${reset_color}"
}
function info {
    printf "%s%s%s\n" "${info_color}" "$1" "${reset_color}"
}
function dialog {
    printf "%s%s%s\n" "${dialog_color}" "$1" "${reset_color}"
}
function warn {
    printf "%s%s%s\n" "${warn_color}" "$1" "${reset_color}"
}
function success {
    printf "%s%s%s\n" "${success_color}" "$1" "${reset_color}"
}

# git info
if ! git diff --staged --quiet || ! git diff --quiet; then
    warn "please commit or stash changes before running bumper"
    exit 1
fi
if ! ROOT=$(git rev-parse --show-toplevel 2> /dev/null); then
    warn "not a git repository"
    exit 1
fi
if ! BRANCH=$(git rev-parse --abbrev-ref HEAD 2> /dev/null); then
    warn "not on a branch"
    exit 1
fi
if ! LAST_HASH=$(git rev-list --tags --max-count=1 2> /dev/null); then
    warn "no git tags found, please create a tag first"
    exit 1
fi
if ! LAST_VERSION=$(git describe --tags "${LAST_HASH}" 2> /dev/null); then
    warn "no git tags found, please create a tag first"
    exit 1
fi

cd "${ROOT}" || exit 1

# get semver impact
readarray -t commits < <(git log --pretty=format:"%s" "${LAST_HASH}..HEAD")
impact="patch"
for commit in "${commits[@]}"; do
    if [[ "${commit}" == feat* ]]; then
        if [[ "${impact}" == "patch" ]]; then
            impact="minor"
        fi
    elif [[ "${commit}" == BREAKING\ CHANGE* ]]; then
        if [[ "${impact}" != "minor" ]]; then
            impact="major"
        fi
    fi
done

bold "$(info "detected impact: ${impact}")"

# get next version
version=${LAST_VERSION#v}
major=$(echo "${version}" | cut -s -d . -f 1)
minor=$(echo "${version}" | cut -s -d . -f 2)
patch=$(echo "${version}" | cut -s -d . -f 3)
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
next_version="${major}.${minor}.${patch}"

bold "$(info "${version} -> ${next_version}")"

# perform automatic bumps
readarray -t search < <(git ls-files)
for file in "${search[@]}"; do
    case "${file}" in
        # node
        "package.json" | "package-lock.json")
            if err=$(npm version "${next_version}" --no-git-tag-version --allow-same-version 2>&1 >/dev/null); then
                git add package.json
                git add package-lock.json
            else
                bold "$(warn "npm version failed")"
                warn "${err}"
            fi
            ;;

        # nix
        "flake.nix")
            if err=$(nix-update --flake --version "${next_version}" default 2>&1 >/dev/null); then
                git add flake.nix
            else
                bold "$(warn "nix-update failed")"
                warn "${err}"
            fi
            ;;
    esac
done

# get files to manually bump
if [[ $# -gt 1 ]]; then
    shift
    readarray -t files <<< "$@"
else
    files=()
fi

# perform manual bumps
for file in "${files[@]}"; do
    # check if file exists
    if [[ ! -f "${file}" ]]; then
        warn "file not found: ${file}"
        continue
    fi

    # look for version occurrences
    readarray -t lines < <(grep -F "${version}" "${file}")
    if [[ ${#lines[@]} -eq 0 ]]; then
        continue
    fi

    # display occurrences
    bold "$(info "${file}")"
    for line in "${lines[@]}"; do
        trim=$(echo "${line}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
        info "${trim}"
    done

    # change version
    sed -i "s/${version}/${next_version}/g" "${file}"

    # validate change
    if grep -q "${next_version}" "${file}"; then
        git add "${file}"
    else
        warn "failed to replace version in ${file}"
        continue
    fi
done

# check for staged changes
if git diff --staged --quiet; then
    bold "$(warn "no changes to commit")"
    exit 1
fi

echo

# commit and tag
git commit -m "bump: v${version} -> v${next_version}"
git tag -a "v${next_version}" -m "bump: v${version} -> v${next_version}"

# done
git push --atomic origin "${BRANCH}" "v${next_version}"
