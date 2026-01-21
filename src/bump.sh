#!/usr/bin/env bash

function bump_dir() {
    local dir="$1"
    local last_version="$2"
    local next_version="$3"

    local search=()
    readarray -t search < <(git ls-files "${dir}")
    for file in "${search[@]}"; do
        case "${file}" in
            # node
            ?(*/)package.json)
                info "bumping: $(bold "${file}")"

                if ! pushd "$(dirname "${file}")" &> /dev/null; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                if run npm version "${next_version}" --no-git-tag-version --allow-same-version; then
                    git add package.json
                    git add package-lock.json || true
                else
                    warn "$(bold "'npm version' failed")"
                fi

                popd &> /dev/null || true
                ;;

            # nix
            ?(*/)flake.nix)
                info "bumping: $(bold "${file}")"

                if ! pushd "$(dirname "${file}")" &> /dev/null; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                local system
                local packages=()

                system=$(nix_system)
                readarray -t packages < <(nix_packages "${system}")
                if [[ ${#packages[@]} -eq 0 ]]; then
                    warn "no packages found in '${file}' for system '${system}'"
                    continue
                fi

                for package in "${packages[@]}"; do
                    if ! run nix-update --flake --version "${next_version}" "${package}"; then
                        warn "'nix-update' failed for package '${package}'"
                    fi
                done

                git add flake.nix

                popd &> /dev/null || true
                ;;

            # rust
            ?(*/)Cargo.toml)
                info "bumping: $(bold "${file}")"

                if ! pushd "$(dirname "${file}")" &> /dev/null; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                if run cargo-set-version set-version "${next_version}"; then
                    git add Cargo.toml
                    git add Cargo.lock || true
                else
                    warn "$(bold "'cargo-set-version' failed")"
                fi

                popd &> /dev/null || true
                ;;

            # python
            ?(*/)uv.lock)
                info "bumping: $(bold "${file}")"

                if ! pushd "$(dirname "${file}")" &> /dev/null; then
                    warn "could not change directory to $(dirname "${file}")"
                    continue
                fi

                if run uv version "${next_version}"; then
                    git add uv.lock
                    git add pyproject.toml || true
                else
                    warn "$(bold "'uv version' failed")"
                fi

                popd &> /dev/null || true
                ;;

            # default
            *)
                # only check all files in interactive mode
                if [[ $- != *i* ]]; then
                    continue
                fi

                # check if file contains the version
                if ! grep -q "${next_version}" "${file}"; then
                    continue
                fi

                info "bump $(bold "${file}")? (y/n): "
                read -r answer
                case "${answer,,}" in
                    y | yes)
                        bump_file "${file}" "${last_version}" "${next_version}"
                        ;;
                    *)
                        info "skipped: $(bold "${file}")"
                        ;;
                esac
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
    sed -i -e "s/${last_version}/${next_version}/g" "${file}"

    # validate change
    if grep -q "${next_version}" "${file}"; then
        git add "${file}"
    else
        warn "failed to replace version in ${file}"
    fi
}
