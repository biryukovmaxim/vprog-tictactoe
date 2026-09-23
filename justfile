# Canonical entry points for formatting, linting and tests.

# List available recipes.
default:
    @just --list

# Format all Rust and TOML files: the workspace plus the excluded guest crate.
fmt:
    @if ls -d node driver >/dev/null 2>&1; then cargo +nightly fmt; fi
    taplo fmt
    @if [ -d guest ]; then \
        cargo +nightly fmt --manifest-path guest/Cargo.toml; \
        taplo fmt guest/Cargo.toml; \
    fi

# Like `just fmt`, but verify only: exit non-zero if anything would change.
fmt-check:
    @if ls -d node driver >/dev/null 2>&1; then cargo +nightly fmt -- --check; fi
    taplo fmt --check
    @if [ -d guest ]; then \
        cargo +nightly fmt --manifest-path guest/Cargo.toml -- --check; \
        taplo fmt --check guest/Cargo.toml; \
    fi

# Format check plus clippy with warnings denied, on the workspace, the guest crate (host
# target), and the guest's zkVM ELF target (rzup `risc0` toolchain; run `rzup install` once).
# One command: `just check` covers formatting, linting and both guest targets.
check: fmt-check
    @if ls -d node driver >/dev/null 2>&1; then cargo clippy --tests -- -D warnings; fi
    @if [ -d guest ]; then \
        cargo clippy --tests --manifest-path guest/Cargo.toml -- -D warnings && \
        cd guest && cargo +risc0 check --target riscv32im-risc0-zkvm-elf; \
    fi

# Build the guest zkVM ELF (release). The reproducible Docker build lands with the ELF
# milestone; this local build is for dev iteration.
build-guest:
    @cd guest && cargo +risc0 build --release --target riscv32im-risc0-zkvm-elf && \
        mkdir -p compiled && \
        cp target/riscv32im-risc0-zkvm-elf/release/vprog-tictactoe-guest compiled/program.elf && \
        ls -la compiled/program.elf

# Build the two vendored wasm npm tarballs into web/vendor (gitignored build
# outputs; CI builds the same): the encoder from this repo's encoder-wasm
# crate, and kaspa-wasm from rusty-kaspa at the rev pinned in Cargo.lock.
# Requires wasm-pack and the wasm32-unknown-unknown rustup target. The rusty
# checkout lives in the user cache dir, outside the repo tree, so source
# sweeps over the repo (format checks and friends) never see it.
web-vendor:
    #!/bin/sh
    set -eu
    mkdir -p web/vendor
    wasm-pack build encoder-wasm --release --target web
    v=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' encoder-wasm/pkg/package.json | head -1)
    tar -C encoder-wasm/pkg -czf "web/vendor/vprog-tictactoe-encoder-wasm-$v.tgz" \
        --transform 's,^\.,package,' .
    rev=$(sed -n 's/.*rusty-kaspa?rev=\([0-9a-f]\{40\}\).*/\1/p' Cargo.lock | head -1)
    dir="${XDG_CACHE_HOME:-$HOME/.cache}/vprog-tictactoe-web-vendor/rusty-kaspa-$rev"
    if [ ! -d "$dir/wasm" ]; then
        mkdir -p "$dir"
        git -C "$dir" init -q
        git -C "$dir" remote add origin https://github.com/kaspanet/rusty-kaspa
        git -C "$dir" fetch -q --depth 1 origin "$rev"
        git -C "$dir" checkout -q FETCH_HEAD
    fi
    wasm-pack build "$dir/wasm" --release --target web --features wasm32-core,wasm32-rpc
    # The pinned rev names the wasm lib kaspa_wasm, but the app and its tests
    # resolve the package artifacts as kaspa.js / kaspa_bg.wasm (the naming
    # upstream ships on npm), so restore those names after the build.
    for f in "$dir/wasm/pkg"/kaspa_wasm*; do
        mv "$f" "${f%kaspa_wasm*}kaspa${f#*kaspa_wasm}"
    done
    sed -i 's/kaspa_wasm/kaspa/g' "$dir/wasm/pkg/kaspa.js" "$dir/wasm/pkg/package.json"
    kv=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$dir/wasm/pkg/package.json" | head -1)
    tar -C "$dir/wasm/pkg" -czf "web/vendor/kaspa-wasm-$kv.tgz" \
        --transform 's,^\.,package,' .
    echo "web/vendor: vprog-tictactoe-encoder-wasm-$v.tgz kaspa-wasm-$kv.tgz"

# Find unused dependencies (nightly toolchain required).
udeps:
    @if ls -d node driver >/dev/null 2>&1; then cargo +nightly udeps --all-targets; fi
    @if [ -d guest ]; then \
        cargo +nightly udeps --all-targets --manifest-path guest/Cargo.toml; \
    fi

# Run workspace and guest crate tests. L1-dependent e2e tests are env-gated and off by default.
test:
    @if ls -d node driver >/dev/null 2>&1; then cargo test; fi
    @if [ -d guest ]; then \
        cargo test --manifest-path guest/Cargo.toml; \
    fi
