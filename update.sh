#!/usr/bin/env bash
# Rebuild this checkout and install it for the current Linux user.
#
# Usage:
#   ./update.sh
#
# PREFIX changes the binary location (default: ~/.local).
# XDG_DATA_HOME changes where the launcher and icon are installed.
#
# The installed binary, launcher and icon are replaced atomically. A running
# Spotifast keeps its current process and needs a restart to use the new build.

set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$root"

if [ "$(uname -s)" != "Linux" ]; then
    echo "update.sh installs the Linux desktop build. Use the platform packaging for macOS or Windows." >&2
    exit 1
fi

prefix=${PREFIX:-$HOME/.local}
data_home=${XDG_DATA_HOME:-$HOME/.local/share}
bin_dir=$prefix/bin
apps_dir=$data_home/applications
icon_dir=$data_home/icons/hicolor/scalable/apps
desktop_source=$root/packaging/applications/spotifast.desktop
icon_source=$root/packaging/icons/spotifast.svg

if command -v mise >/dev/null 2>&1; then
    cargo=(mise exec -- cargo)
else
    cargo=(cargo)
fi

echo "Building Spotifast from $root..."
"${cargo[@]}" build --locked --release

binary=$root/target/release/spotifast
if [ ! -x "$binary" ]; then
    echo "The release build did not produce $binary." >&2
    exit 1
fi

install -d "$bin_dir" "$apps_dir" "$icon_dir"

tmp_bin=
tmp_desktop=
tmp_icon=
cleanup() {
    rm -f -- "${tmp_bin:-}" "${tmp_desktop:-}" "${tmp_icon:-}"
}
trap cleanup EXIT

# Install beside the destination and rename over it, so an interrupted update
# never leaves a truncated binary, launcher or icon.
tmp_bin=$(mktemp "$bin_dir/.spotifast.XXXXXX")
install -m 0755 "$binary" "$tmp_bin"
mv -f -- "$tmp_bin" "$bin_dir/spotifast"
tmp_bin=

tmp_desktop=$(mktemp "$apps_dir/.spotifast-desktop.XXXXXX")
sed "s|^Exec=spotifast |Exec=$bin_dir/spotifast |" "$desktop_source" > "$tmp_desktop"
chmod 0644 "$tmp_desktop"
mv -f -- "$tmp_desktop" "$apps_dir/spotifast.desktop"
tmp_desktop=

tmp_icon=$(mktemp "$icon_dir/.spotifast-icon.XXXXXX")
install -m 0644 "$icon_source" "$tmp_icon"
mv -f -- "$tmp_icon" "$icon_dir/spotifast.svg"
tmp_icon=

# Refresh the desktop's caches when the tools are present. A missing tool is
# not an update failure; the launcher still reads the files directly.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data_home/icons/hicolor" >/dev/null 2>&1 || true
fi
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 >/dev/null 2>&1 || true
elif command -v kbuildsycoca5 >/dev/null 2>&1; then
    kbuildsycoca5 >/dev/null 2>&1 || true
fi

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$apps_dir/spotifast.desktop"
fi

printf 'Installed %s\n' "$("$bin_dir/spotifast" --version)"
printf 'Launcher: %s\n' "$apps_dir/spotifast.desktop"
printf 'Restart Spotifast to use the new build.\n'
