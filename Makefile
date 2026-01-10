.PHONY: all build check test fmt clippy deny doc clean install dev setup

# Default target
all: check test

# Build release binary
build:
	cargo build --release

# Quick check (no codegen)
check:
	cargo check --workspace --all-targets

# Run all tests
test:
	cargo test --workspace --all-features

# Format code
fmt:
	cargo fmt --all

# Check formatting
fmt-check:
	cargo fmt --all --check

# Run clippy
clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run cargo-deny checks
deny:
	cargo deny check

# Generate documentation
doc:
	cargo doc --workspace --no-deps --all-features

# Clean build artifacts
clean:
	cargo clean

# Install locally
install:
	cargo install --path crates/devdock-cli

# Development build
dev:
	cargo build

# Full CI simulation
ci: fmt-check clippy test deny doc
	@echo "All CI checks passed!"

# Setup development environment
setup:
	@echo "Installing required tools..."
	rustup component add rustfmt clippy
	cargo install cargo-deny
	@command -v pre-commit >/dev/null 2>&1 || echo "Install pre-commit: pip install pre-commit"
	pre-commit install || true
	pre-commit install --hook-type pre-push || true
	@echo "Setup complete!"

# Run pre-commit on all files
pre-commit:
	pre-commit run --all-files

# Watch for changes and rebuild
watch:
	cargo watch -x check

# Show workspace dependency tree
deps:
	cargo tree --workspace

# Update dependencies
update:
	cargo update

# Check for outdated dependencies
outdated:
	cargo outdated --workspace

# Security audit
audit:
	cargo deny check advisories

# Generate code coverage report
coverage:
	cargo llvm-cov --workspace --all-features --html
	@echo "Coverage report: target/llvm-cov/html/index.html"

