# Helpers shared by fork-setup.sh and mainnet-setup.sh. Source it; do not run it.

log() { echo "$@" >&2; }
die() { log "error: $*"; exit 1; }
addr() { cast wallet address --private-key "$1"; }

# A fresh private key. 32 random bytes are a valid secp256k1 key with overwhelming probability.
new_key() { echo "0x$(openssl rand -hex 32)"; }

# Where each crate's .env lives. Overridable so the scripts can be exercised without touching
# the real files.
ENV_ROOT=${ENV_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}

# env_set <crate> <KEY> <value>
#
# Sets KEY=value in <crate>/.env and leaves every other line alone: secrets you added by hand,
# comments, ordering. A missing .env is created from that crate's .env.example first, so the
# documented defaults come along.
env_set() {
  local file="$ENV_ROOT/$1/.env" key=$2 value=$3
  if [ ! -f "$file" ]; then
    if [ -f "$file.example" ]; then cp "$file.example" "$file"; else : > "$file"; fi
  fi
  if grep -qE "^${key}=" "$file"; then
    # escape what sed's replacement treats specially; values here are keys, addresses, URLs
    local v=${value//\\/\\\\}
    v=${v//&/\\&}
    v=${v//|/\\|}
    sed -i.bak -E "s|^${key}=.*$|${key}=${v}|" "$file" && rm -f "$file.bak"
  else
    printf '%s=%s\n' "$key" "$value" >> "$file"
  fi
}

# env_get <crate> <KEY>: the value in <crate>/.env, without any trailing comment.
env_get() {
  local file="$ENV_ROOT/$1/.env"
  [ -f "$file" ] || return 0
  # a missing key is an empty value, not a failure: callers run under `set -e -o pipefail`
  { grep -E "^$2=" "$file" || true; } | tail -1 | cut -d= -f2- | sed -E 's/[[:space:]]+#.*$//; s/^"(.*)"$/\1/'
}
