#!/bin/bash

set -e

echo "checking no features"
cargo check --all-targets --no-default-features

echo "checking all features"
cargo check --all-targets --all-features

for feature in $(cargo metadata --format-version=1 --no-deps | jq -r '.packages[] | select(.name = "arcu") | .features | keys[]') ; do
    echo "checking features '${feature}'"
    cargo test --all-targets --features="${feature}"
done