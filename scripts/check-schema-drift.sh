#!/bin/sh
set -eu
cargo run --quiet -p shardrail-protocol --bin export-schema
npm --prefix packages/contracts run generate
git diff --exit-code -- schema/contracts.schema.json packages/contracts/src/generated.ts
