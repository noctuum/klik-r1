#!/usr/bin/env bash
# Build the release binary, install it for the user service and restart it.
#
# The binary is copied beside the installed one and renamed over it: a copy
# over a running binary fails with "Text file busy". The unit is reloaded
# because a checkout changes the file its symlink points at.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release
install -m 755 target/release/klik "$HOME/.local/bin/klik.new"
mv -f "$HOME/.local/bin/klik.new" "$HOME/.local/bin/klik"
systemctl --user daemon-reload
systemctl --user restart klik.service
systemctl --user --no-pager --lines=0 status klik.service
journalctl --user -u klik.service -o cat -n 2 --no-pager
