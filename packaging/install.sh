#!/bin/sh
set -eu
case "$(uname -s)" in
    Linux) ;;
    *) printf '%s\n' 'This installer only runs on Linux; Windows host installation is forbidden.' >&2; exit 1 ;;
esac
if [ "$(id -u)" -eq 0 ]; then
    printf '%s\n' 'Start as your normal desktop user, not sudo/root.' >&2
    exit 1
fi
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
exec python3 "$base/install.py" --bundle "$base" "$@"
