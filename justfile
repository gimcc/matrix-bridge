set shell := ["bash", "-cu"]

crates := "matrix-bridge matrix-bridge-core matrix-bridge-appservice matrix-bridge-store"

# Run the bridge with default config
run *args:
    cargo run -- {{args}}

# Format all crates
fmt:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo fmt -p "$c"
    done

# Check formatting
fmt-check:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo fmt -p "$c" -- --check
    done

# Run clippy on all crates
clippy:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo clippy -p "$c" --all-targets -- -D warnings
    done

# Run cranky (strict clippy) on all crates
cranky:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo cranky -p "$c" --all-targets
    done

# Run tests
test:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo test -p "$c"
    done

# Build in release mode
build-release:
    cargo build --release

# Run cargo deny checks
deny:
    cargo deny check

# Run all quality gates (pre-merge check)
check:
    just fmt-check
    just cranky
    just deny
    just test
    just build-release

# Check docs build without warnings
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Full cleanup: fix -> fmt -> clippy -> test
cleanup:
    #!/usr/bin/env bash
    for c in {{crates}}; do
        cargo fix -p "$c" --allow-dirty --allow-staged
    done
    just fmt
    just clippy
    just test

# Check for unused dependencies
udeps:
    cargo +nightly udeps --all-targets
