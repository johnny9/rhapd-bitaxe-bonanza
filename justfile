# Recipes for building and flashing the firmware. Run `just` to list
# them.

ESPUP_VERSION := "0.17.1"
ESPFLASH_VERSION := "4.4.0"
ESP_RUST_VERSION := "1.93.0.0"
ESP_TOOLCHAIN := "esp-" + ESP_RUST_VERSION

_default:
    @just --list --unsorted

# Install espup, the esp toolchain, and espflash
[group('setup')]
setup-tools:
    @if espup --version 2>/dev/null | grep -q " {{ESPUP_VERSION}}$"; then \
        echo "espup {{ESPUP_VERSION}} already installed"; \
    else \
        cargo +stable install --locked --force espup --version {{ESPUP_VERSION}}; \
    fi
    @if rustc +{{ESP_TOOLCHAIN}} --version 2>/dev/null | grep -q "({{ESP_RUST_VERSION}})"; then \
        echo "esp toolchain {{ESP_RUST_VERSION}} already installed"; \
    else \
        f=$(mktemp) \
        && { espup install --name {{ESP_TOOLCHAIN}} --targets esp32s3 --toolchain-version {{ESP_RUST_VERSION}} --export-file "$f"; \
             s=$?; rm -f "$f"; exit $s; }; \
    fi
    @if command -v cargo-espflash >/dev/null; then \
        echo "cargo-espflash already installed; using the system copy"; \
    else \
        cargo +stable install --locked cargo-espflash --version {{ESPFLASH_VERSION}}; \
    fi

[group('dev')]
fmt *args:
    cargo fmt {{args}}

[group('dev')]
lint:
    cargo clippy --release --locked -- -D warnings

# Run all checks (before commit, push, merge, release)
[group('dev')]
@checks: (fmt "--check") lint test

# Run hardware-independent framing, safety, fan, bridge and flash tests
[group('dev')]
test:
    tools/test-host

# Build the release firmware
[group('dev')]
build:
    cargo build --release --locked

# Build and flash the release firmware over USB
[group('dev')]
flash: (_require "cargo-espflash")
    tools/board bootloader
    cargo espflash flash --release --locked --chip esp32s3 --after no-reset
    tools/board reset

[private]
_require tool:
    @command -v {{tool}} >/dev/null || { \
        echo "error: {{tool}} is not installed; run 'just setup-tools'" >&2; \
        exit 1; }
