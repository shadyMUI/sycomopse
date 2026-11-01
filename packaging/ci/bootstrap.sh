#!/bin/sh
set -eu
if [ "$(uname -s)" != Linux ] || [ "$(id -u)" -ne 0 ] || [ "${PH_CI_CONTAINER:-}" != 1 ] || [ ! -f /.dockerenv ]; then
    printf '%s\n' 'Dependency setup is restricted to the explicit Linux CI build container.' >&2
    exit 1
fi
target=$1
case "$target" in
    arch|cachyos)
        pacman -Syu --noconfirm --needed python python-setuptools python-build base-devel git curl ca-certificates sudo
        ;;
    ubuntu-*|debian-*)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update
        apt-get install --yes --no-install-recommends python3 python3-dev python3-setuptools build-essential git curl ca-certificates sudo
        ;;
    fedora-*)
        dnf install --assumeyes python3 python3-devel python3-setuptools gcc git curl ca-certificates sudo dnf-plugins-core
        ;;
    opensuse-*)
        zypper --non-interactive refresh
        zypper --non-interactive install python3 python3-devel python3-setuptools gcc git curl ca-certificates sudo
        ;;
    *) printf '%s\n' 'Unknown build target' >&2; exit 1 ;;
esac
python3 /opt/peripheral-hub-packaging/ci/dependencies.py --target "$target"
