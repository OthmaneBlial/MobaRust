#!/bin/sh
# Runs every localhost integration suite that uses real protocol traffic on
# this machine, then checks that no fixture daemons were left behind.
# Uses only tools already on macOS/Linux (/usr/sbin/sshd, ssh-keygen, PTYs).
# No Docker, VMs or extra services. See docs/TEST_MATRIX.md.
set -eu

REPOSITORY_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPOSITORY_ROOT"

if [ ! -x /usr/sbin/sshd ] && [ ! -x /usr/local/sbin/sshd ]; then
  echo "sshd not found; real SSH suites cannot run on this machine" >&2
  exit 1
fi

LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT
run() {
  if ! cargo test --locked "$@" -- --nocapture >"$LOG" 2>&1; then
    grep -E 'FAILED|panicked|error' "$LOG" | head -40 >&2
    exit 1
  fi
  grep -E 'test result|14-session group' "$LOG"
}

echo "== SSH, SFTP, SCP, tunnels, remote editing, 14-session group (loopback sshd)"
run -p mobarust-ssh
echo "== Telnet (loopback fixture server), serial (PTY pairs), network diagnostics"
run -p mobarust-telnet -p mobarust-serial -p mobarust-network
echo "== Desktop managers (local PTY, SSH manager, backpressure, shutdown)"
run -p mobarust

leftover=$(pgrep -f 'sshd -D -e -f' || true)
if [ -n "$leftover" ]; then
  echo "fixture sshd processes left behind: $leftover" >&2
  exit 1
fi
echo "cleanup: no fixture sshd processes remain"
