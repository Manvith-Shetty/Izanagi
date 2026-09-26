#!/usr/bin/env bash
# Copy the compiled creation code of the contracts Tab deploys into tab/assets/, so the Tab
# server can deploy a wallet per person without Foundry installed where it runs.
# Rerun after changing contracts/src/Countersign.sol; tab's tests fail if the copy is stale.
set -euo pipefail
cd "$(dirname "$0")/../contracts"
forge build --silent
for c in Countersign CountersignCollector; do
  jq -r .bytecode.object "out/Countersign.sol/$c.json" > "../tab/assets/$c.bin"
  echo "tab/assets/$c.bin  $(wc -c < "../tab/assets/$c.bin" | tr -d ' ') hex chars" >&2
done
