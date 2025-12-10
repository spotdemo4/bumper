#!/usr/bin/env bash

function bump_dir() {
    local dir="$1"
    local next_version="$2"

    local search=()
    readarray -t search < <(git ls-files "${dir}")
    for file in "${search[@]}"; do
        case "${file}" in
            # node
            ?(*/)package.json)
                info "bumping: $(bold "${file}")"

                if ! run pushd "$(dirname "${file}")"; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                if run npm version "${next_version}" --no-git-tag-version --allow-same-version; then
                    run git add package.json
                    run git add package-lock.json || true
                else
                    warn "$(bold "'npm version' failed")"
                fi

                run popd || true
                ;;

            # nix
            ?(*/)flake.nix)
                info "bumping: $(bold "${file}")"

                if ! run pushd "$(dirname "${file}")"; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                local system
                local packages=()

                system=$(nix_system)
                readarray -t packages < <(nix_packages "${system}")
                if [[ ${#packages[@]} -eq 0 ]]; then
                    warn "no packages found in '${file}' for system '${system}'"
                    exit 1
                fi

                for package in "${packages[@]}"; do
                    if ! run nix-update --flake --version "${next_version}" "${package}"; then
                        warn "'nix-update' failed for package '${package}'"
                    fi
                done

                run git add flake.nix

                run popd || true
                ;;

            # rust
            ?(*/)Cargo.toml)
                info "bumping: $(bold "${file}")"

                if ! run pushd "$(dirname "${file}")"; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                if run cargo-set-version set-version "${next_version}"; then
                    run git add Cargo.toml
                    run git add Cargo.lock || true
                else
                    warn "$(bold "'cargo-set-version' failed")"
                fi

                run popd || true
                ;;
        esac
    done
}

function bump_file() {
    local file="$1"
    local last_version="$2"
    local next_version="$3"

    # look for version occurrences
    local lines=()
    readarray -t lines < <(grep -F "${last_version}" "${file}")
    if [[ ${#lines[@]} -eq 0 ]]; then
        warn "no occurrences found in ${file}"
        return
    fi

    # display file being changed
    info "bumping: $(bold "${file}")"

    # change version
    sd -F "${last_version}" "${next_version}" "${file}"

    # validate change
    if grep -q "${next_version}" "${file}"; then
        git add "${file}"
    else
        warn "failed to replace version in ${file}"
    fi
}
