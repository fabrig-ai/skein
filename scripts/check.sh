#!/bin/sh
# What CI runs: formatting, lints as errors, then both test suites in full
# (testing-strategy.md, section 8): the focused one, then the fuzzy one.
# Nothing slower: the browser tier is scripts/browser.sh, outside the gate
# (agents' docs/design/landing-flow.md, 4).
set -eu

cd "$(dirname "$0")/.."

# Each step's time, for the landing flow's reports (agents'
# docs/design/landing-flow.md, 10): `gate-step NAME SECONDS EXIT` on stderr.
step() {
    step_name=$1
    shift
    step_start=$(date +%s%N)
    step_exit=0
    "$@" || step_exit=$?
    step_ms=$((($(date +%s%N) - step_start) / 1000000))
    printf 'gate-step %s %d.%03d %d\n' "$step_name" $((step_ms / 1000)) $((step_ms % 1000)) "$step_exit" >&2
    return "$step_exit"
}

# programming-model.md, 2: every binary installs the fail-stop hook first.
step fail-stop python3 -I -B scripts/fail_stop.py
step fmt cargo fmt --all --check
# testing-strategy.md 10: A setup written once; Named fixtures and sizes.
mkdir -p target
rustc --edition=2024 -D warnings scripts/check-contracts.rs -o target/check-contracts
target/check-contracts
# testing-strategy.md 10: A setup written once; The setup the assertion reads; Named fixtures and sizes.
cargo run --locked --target-dir target/recovery-shape --manifest-path scripts/recovery-shape/Cargo.toml -- --controls crates/skein-forge-client/src/tests/recovery.rs crates/skein-forge-client/src/tests/fixtures/recovery.rs
# testing-strategy.md 10: One behaviour per test, in the four sampled plan tests.
cargo run --locked --target-dir target/recovery-shape --manifest-path scripts/recovery-shape/Cargo.toml -- --plan-shapes --controls crates/skein-forge/src/forgejo/plans/tests/edges.rs
# testing-strategy.md 10: sampled 03.3 behaviours and capacities.
cargo run --locked --target-dir target/recovery-shape --manifest-path scripts/recovery-shape/Cargo.toml -- --connection-shapes --controls crates/skein-forge-connection/src/tests/connection.rs crates/skein-forge-connection/src/limits/tests.rs
# testing-strategy.md 10: Named fixtures and sizes, in the two sampled listing tests.
cargo run --locked --target-dir target/recovery-shape --manifest-path scripts/recovery-shape/Cargo.toml -- --listing-sizes --controls crates/skein-forge/src/forgejo/plans/tests/calls.rs
step clippy cargo clippy --workspace --all-targets -- -D warnings
# The portable libraries and the browser-only halves on their shipping target.
step wasm cargo check --target wasm32-unknown-unknown -p skein-lib -p skein-codec -p skein-http
step wasm cargo clippy --target wasm32-unknown-unknown -p skein-ui -p skein-ui-web \
    -p skein-ui-example-view -p skein-ui-example-shell-web -- -D warnings
# TLS needs ring's custom entropy backend on wasm; GitHub's layer links TLS.
# The peers also need their buffered rustls transport made portable.
step wasm-portable cargo clippy --target wasm32-unknown-unknown \
    -p skein-lib -p skein-io -p skein-sim -p skein-shell -p skein-fake-machine -p skein-sim-world -p skein-playground \
    -p skein-kv -p skein-json -p skein-http -p skein-channel -p skein-codec \
    -p skein-fake-llm-domain -p skein-fake-llm-protocol -p skein-fake-forge-domain \
    -p skein-fake-forge-protocol -- -D warnings
scripts/taste.sh crates/skein-tls tests/tls
# The TLS machine on its own: the workspace's build unifies rustls's std,
# which tests/tls turns on, and would hide a use of it.
cargo check -p skein-tls
# testing-strategy.md, section 8: nextest starts each test directly.
export NEXTEST_DOUBLE_SPAWN=0
step build cargo nextest run --workspace --no-run
step focused cargo nextest run --workspace
step fuzzy cargo nextest run --workspace --profile fuzzy
