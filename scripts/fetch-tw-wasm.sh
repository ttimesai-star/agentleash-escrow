#!/usr/bin/env sh
# Fetch the Trustless Work single-release escrow wasm from Stellar testnet by its hash.
# The wasm is NOT committed (upstream repo has no LICENSE); it is used only to build tests and type bindings.
set -e
HASH=7c3f7b2af92ad86092708b23babf80f9e1308d7f3ce18b703b9499192ecc934b
mkdir -p tw
stellar contract fetch --wasm-hash "$HASH" --network testnet -o tw/tw_escrow.wasm
echo "$HASH  tw/tw_escrow.wasm" | sha256sum -c -
