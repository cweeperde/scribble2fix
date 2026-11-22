#!/usr/bin/env bash
# Usage: release-notes.sh 1.2.3 CHANGELOG.md
# Prints the text under "## 1.2.3 (...)" up to the next "## " heading,
# without leading and trailing blank lines. Fails if it's empty.
set -euo pipefail
notes=$(awk -v h="## $1 (" '
  index($0, h) == 1 { found = 1; next }
  found && /^## / { exit }
  found { print }
' "$2" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}' | sed '/./,$!d')
if [ -z "$notes" ]; then
  echo "::error::CHANGELOG.md has no text for $1" >&2
  exit 1
fi
printf '%s\n' "$notes"
