#!/usr/bin/env bash
# Install (or --uninstall) the Finder KRunner plugin for the current user.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
plugin="$data/krunner/dbusplugins/finder.desktop"
service="$data/dbus-1/services/org.finder.krunner.service"
dest="$data/finder/krunner"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$plugin" "$service"; rm -rf "$dest"
    pkill -f "$dest/finder-runner.py" 2>/dev/null || true
    echo "Removed. Restart KRunner: kquitapp6 krunner"; exit 0
fi

finder="${FINDER_BIN:-$here/../../target/release/finder}"
finder="$(readlink -f "$finder")"
[ -x "$finder" ] || { echo "finder binary not found: $finder (run cargo build --release)"; exit 1; }

mkdir -p "$(dirname "$plugin")" "$(dirname "$service")" "$dest"
install -m755 "$here/finder-runner.py" "$dest/finder-runner.py"

cat > "$plugin" <<EOT
[Desktop Entry]
Type=Service
Name=Finder
Comment=Search for files and folders by name with Finder
Icon=system-search
X-KDE-ServiceTypes=Plasma/Runner
X-Plasma-API=DBus
X-Plasma-DBusRunner-Service=org.finder.krunner
X-Plasma-DBusRunner-Path=/runner
EOT

cat > "$service" <<EOT
[D-BUS Service]
Name=org.finder.krunner
Exec=/usr/bin/python3 $dest/finder-runner.py --finder $finder
EOT

echo "Installed. Restart KRunner: kquitapp6 krunner"
