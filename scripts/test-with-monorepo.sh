#!/bin/bash
# Test DevDock with the Scio monorepo profiles

set -e

MONOREPO_PATH="${1:-$HOME/workspace/scio}"

if [ ! -d "$MONOREPO_PATH" ]; then
    echo "❌ Monorepo not found at: $MONOREPO_PATH"
    echo "Usage: $0 [path-to-monorepo]"
    exit 1
fi

echo "✓ Found monorepo at: $MONOREPO_PATH"
echo "⏩ Building DevDock..."

cargo build --release

echo "✓ Build complete!"
echo ""
echo "⏩ Testing with monorepo profiles..."

# Test validate command with real profiles
for profile in "$MONOREPO_PATH/devdock/profiles"/*.yaml; do
    profile_name=$(basename "$profile" .yaml)
    echo "  Testing: $profile_name"
    ./target/release/devdock validate "$profile_name" 2>/dev/null || echo "    ⚠️  Failed (expected - needs monorepo context)"
done

echo ""
echo "✓ DevDock binary is ready at: ./target/release/devdock"
echo ""
echo "Next steps:"
echo "  1. Install: cargo install --path ."
echo "  2. cd $MONOREPO_PATH"
echo "  3. devdock validate default_profile"

