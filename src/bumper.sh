#!/usr/bin/env bash
# export PATH="${PATH}" placeholder, will be replaced in release

set -e

# make source imports work
DIR="${BASH_SOURCE%/*}"
if [[ ! -d "$DIR" ]]; then DIR="$PWD"; fi

source "$DIR/colors.sh"
source "$DIR/git.sh"
source "$DIR/util.sh"

# validate the git environment is set up correctly
if ! git diff --staged --quiet || ! git diff --quiet; then
    warn "please commit or stash changes before running bumper"
    exit 1
fi
if ! git fetch --all --tags --quiet; then
    warn "could not fetch commits and tags"
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

# go to repo root
cd "${ROOT}" || exit 1

# get vars from env
readarray -t SKIP_SCOPES <<< "${SKIP_SCOPES:-"ci"}"
readarray -t MAJOR_TYPES <<< "${MAJOR_TYPES:-"BREAKING CHANGE"}"
readarray -t MINOR_TYPES <<< "${MINOR_TYPES:-"feat"}"
readarray -t PATCH_TYPES <<< "${PATCH_TYPES:-"fix"}"

# check if we should force a version bump
IMPACT=""
if [[ "${FORCE:-false}" == "true" ]]; then
    warn "FORCE is true, forcing (at least) a PATCH version bump"
    IMPACT="patch"
fi

# get semver impact from commits
# https://www.conventionalcommits.org/en/v1.0.0/
readarray -t COMMITS < <(git log --pretty=format:"%s" "${LAST_HASH}..HEAD")
bold "$(info "checking ${#COMMITS[@]} commits since last tag (${LAST_VERSION})")"
for COMMIT in "${COMMITS[@]}"; do
    # skip commits that don't follow conventional commit format
    if [[ ! "${COMMIT}" == *:* ]]; then
        info "skipped (convention): ${COMMIT}"
        continue
    fi

    PREFIX=$(echo "${COMMIT}" | cut -d ':' -f 1)
    TYPE=$(echo "${PREFIX}" | cut -d '(' -f 1)
    SCOPE=$(echo "${PREFIX}" | cut -s -d '(' -f 2 | cut -s -d ')' -f 1)

    # default empty scope to "none"
    if [[ -z "${SCOPE}" ]]; then
        SCOPE="none"
    fi

    # check if scope is in skip list
    for SKIP_SCOPE in "${SKIP_SCOPES[@]}"; do
        if [[ "${SCOPE,,}" == "${SKIP_SCOPE,,}" ]]; then
            info "skipped (scope): ${COMMIT}"
            continue 2
        fi
    done

    # if commit prefix ends with "!", it's a major change
    if [[ "${PREFIX: -1}" == "!" ]]; then
        info "$(bold "major:") ${COMMIT}"
        IMPACT="major"
        break
    fi

    for MAJOR_TYPE in "${MAJOR_TYPES[@]}"; do
        if [[ "${TYPE,,}" == "${MAJOR_TYPE,,}" ]]; then
            info "$(bold "major:") ${COMMIT}"
            IMPACT="major"
            break 2
        fi
    done

    for MINOR_TYPE in "${MINOR_TYPES[@]}"; do
        if [[ "${TYPE,,}" == "${MINOR_TYPE,,}" ]]; then
            info "$(bold "minor:") ${COMMIT}"
            IMPACT="minor"
            continue 2
        fi
    done

    # skip checking for patches if already impactful
    if [[ -n "${IMPACT}" ]]; then
        info "skipped (impact): ${COMMIT}"
        continue
    fi

    for PATCH_TYPE in "${PATCH_TYPES[@]}"; do
        if [[ "${TYPE,,}" == "${PATCH_TYPE,,}" ]]; then
            info "$(bold "patch:") ${COMMIT}"
            IMPACT="patch"
            continue 2
        fi
    done

    info "skipped (type): ${COMMIT}"
done

if [[ -z "${IMPACT}" ]]; then
    warn "no new impactful commits since last tag (${LAST_VERSION})"
    exit 0
fi

bold "$(info "impact: ${IMPACT}")"

# get next version
VERSION=${LAST_VERSION#v}
major=$(echo "${VERSION}" | cut -s -d . -f 1)
minor=$(echo "${VERSION}" | cut -s -d . -f 2)
patch=$(echo "${VERSION}" | cut -s -d . -f 3)
case "${IMPACT}" in
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
NEXT_VERSION="${major}.${minor}.${patch}"

bold "$(info "${VERSION} -> ${NEXT_VERSION}")"

# search for files to bump
readarray -t SEARCH < <(git ls-files)

# validate all required deps are installed
for FILE in "${SEARCH[@]}"; do
    case "${FILE}" in
        # node
        "package.json")
            if ! run command -v npm; then
                bold "$(warn "npm not found")"
                warn "please install npm to bump package.json files"
                exit 2
            fi
            ;;

        # nix
        "flake.nix")
            if ! run command -v nix-update; then
                bold "$(warn "nix-update not found")"
                warn "please install nix-update to bump flake.nix files"
                exit 2
            fi
            ;;

        # rust
        "Cargo.toml")
            if ! run command -v cargo-bump; then
                bold "$(warn "cargo-bump not found")"
                warn "please install cargo-bump to bump Cargo.toml files"
                exit 2
            fi
            ;;
    esac
done

# perform automatic bumps
for FILE in "${SEARCH[@]}"; do
    case "${FILE}" in
        # node
        "package.json")
            if run npm version "${NEXT_VERSION}" --no-git-tag-version --allow-same-version; then
                git add package.json
                git add package-lock.json
            else
                bold "$(warn "'npm version' failed")"
            fi
            ;;

        # nix
        "flake.nix")
            if run nix-update --flake --version "${NEXT_VERSION}" default; then
                git add flake.nix
            else
                bold "$(warn "'nix-update' failed")"
            fi
            ;;

        # rust
        "Cargo.toml")
            if run cargo-bump "${NEXT_VERSION}"; then
                git add Cargo.toml
                git add Cargo.lock
            else
                bold "$(warn "'cargo-bump' failed")"
            fi
            ;;
    esac
done

# get files from args & env
ARG_FILES=("$@")
readarray -t ENV_FILES <<< "${FILES}"
FILES=( "${ARG_FILES[@]}" "${ENV_FILES[@]}" )

# perform manual bumps
for FILE in "${FILES[@]}"; do
    # skip empty args
    if [[ -z "${FILE}" ]]; then
        continue
    fi

    # check if file exists
    if [[ ! -f "${FILE}" ]]; then
        warn "file not found: ${FILE}"
        continue
    fi

    # look for version occurrences
    readarray -t LINES < <(grep -F "${VERSION}" "${FILE}")
    if [[ ${#LINES[@]} -eq 0 ]]; then
        warn "no occurrences found in ${FILE}"
        continue
    fi

    # display file being changed
    info "bumping: $(bold "${FILE}")"

    # change version
    sed -i "s/${VERSION}/${NEXT_VERSION}/g" "${FILE}"

    # validate change
    if grep -q "${NEXT_VERSION}" "${FILE}"; then
        git add "${FILE}"
    else
        warn "failed to replace version in ${FILE}"
        continue
    fi
done

# check for staged changes
if git diff --staged --quiet; then
    bold "$(warn "no changes to commit")"
    exit 1
fi

if [[ "${COMMIT:-true}" == "false" ]]; then
    bold "$(info "COMMIT is false, skipping commit and tag")"
    exit 0
fi

info "committing: v${VERSION} -> v${NEXT_VERSION}"
run git commit -m "bump: v${VERSION} -> v${NEXT_VERSION}"

info "creating tag: v${NEXT_VERSION}"
run git tag -a "v${NEXT_VERSION}" -m "bump: v${VERSION} -> v${NEXT_VERSION}"

if [[ "${PUSH:-true}" == "false" ]]; then
    bold "$(info "PUSH is false, skipping push")"
    exit 0
fi

success "pushing changes to origin ${BRANCH}"
run git push --atomic origin "${BRANCH}" "v${NEXT_VERSION}"
