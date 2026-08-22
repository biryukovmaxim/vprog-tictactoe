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
        cargo clippy --tests --manifest-path guest/Cargo.toml -- -D warnings; \
        cd guest && cargo +risc0 check --target riscv32im-risc0-zkvm-elf; \
    fi

# Build the guest zkVM ELF (release). The reproducible Docker build lands with the ELF
# milestone; this local build is for dev iteration.
build-guest:
    @cd guest && cargo +risc0 build --release --target riscv32im-risc0-zkvm-elf && \
        ls -la target/riscv32im-risc0-zkvm-elf/release/vprog-tictactoe-guest

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
