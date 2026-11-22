#!/bin/sh
set -eu
if [ "$(uname -s)" != Linux ] || [ "$(id -u)" -ne 0 ] || [ "${PH_CI_CONTAINER:-}" != 1 ] || { [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ] && [ ! -f /.containerenv ]; }; then
    printf '%s\n' 'Dependency setup is restricted to the explicit Linux CI build container.' >&2
    exit 1
fi
target=$1
mode=${2:-}
if [ -n "$mode" ] && [ "$mode" != --runtime-only ]; then
    printf '%s\n' 'Unknown dependency setup mode' >&2
    exit 1
fi
if [ "$mode" = --runtime-only ]; then
    case "$target" in
        arch|cachyos) pacman -Syu --noconfirm --needed python ca-certificates ;;
        ubuntu-*|debian-*) export DEBIAN_FRONTEND=noninteractive; apt-get update; apt-get install --yes --no-install-recommends python3 ca-certificates ;;
        fedora-*) dnf install --assumeyes python3 ca-certificates ;;
        opensuse-*) zypper --non-interactive refresh; zypper --non-interactive install python3 ca-certificates ;;
        *) printf '%s\n' 'Unknown runtime target' >&2; exit 1 ;;
    esac
    exec python3 /opt/peripheral-hub-packaging/ci/dependencies.py --target "$target" --runtime-only
fi
case "$target" in
    arch|cachyos)
        pacman -Syu --noconfirm --needed python python-setuptools python-build base-devel git curl ca-certificates sudo
        ;;
    ubuntu-22.04)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update
        apt-get install --yes --no-install-recommends software-properties-common gpg-agent ca-certificates
        add-apt-repository --yes ppa:ubuntu-toolchain-r/test
        apt-get update
        apt-get install --yes --no-install-recommends python3 python3-dev python3-setuptools build-essential g++-13 git curl sudo
        update-alternatives --install /usr/bin/gcc gcc /usr/bin/gcc-13 60 --slave /usr/bin/g++ g++ /usr/bin/g++-13
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
