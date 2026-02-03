# Publishing DevDock

Guide for releasing new versions of DevDock.

## Quick Release Process

### 1. Prepare the Release

```bash
# Update version in Cargo.toml
vim Cargo.toml  # Update workspace.package.version

# Update CHANGELOG.md
vim CHANGELOG.md

# Run tests
cargo test --all
cargo clippy --all-targets --all-features

# Build and test locally
cargo build --release
./target/release/devdock --version
```

### 2. Create and Push Tag

```bash
# Commit changes
git add .
git commit -m "Prepare for vX.Y.Z release"
git push origin main

# Create and push tag
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin vX.Y.Z
```

### 3. Wait for CI/CD

The GitHub Actions workflow (`.github/workflows/release.yml`) will automatically:
- ✅ Build binaries for macOS (ARM64 + Intel) and Linux
- ✅ Generate SHA256 checksums
- ✅ Create a GitHub Release with all artifacts

Monitor the Actions tab: https://github.com/askscio/devdock/actions

### 4. Verify the Release

Once CI completes:

```bash
# Check the release page
open https://github.com/askscio/devdock/releases/latest

# Test the installer
curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh | bash

# Verify it installed correctly
devdock --version
```

### 5. Announce

- Post in Slack/Teams
- Update internal documentation
- Notify users

## Installation Methods

**Note:** The repository is currently **PRIVATE**, which requires authentication.

### 1. Installer Script (Recommended for Internal Users)

```bash
gh repo clone askscio/devdock
cd devdock
./install.sh
```

Features:
- Uses GitHub CLI for authenticated access
- Automatic OS/architecture detection
- Checksum verification
- Version detection and upgrade handling
- PATH checking

### 2. Direct Binary Download (via gh CLI)

```bash
gh release download v0.1.0 --repo askscio/devdock --pattern "devdock-macos-arm64-v0.1.0"
chmod +x devdock-macos-arm64-v0.1.0
sudo mv devdock-macos-arm64-v0.1.0 /usr/local/bin/devdock
```

### 3. Build from Source

```bash
gh repo clone askscio/devdock
cd devdock
cargo install --path crates/devdock-cli
```

### Making the Repository Public (Optional)

If you want to enable simpler installation for external users:

```bash
gh repo edit askscio/devdock --visibility public
```

After making it public, users can use the simpler `install.sh` script:

```bash
curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh | bash
```

See [docs/installation-options.md](installation-options.md) for details.

## Release Checklist

Use `.github/RELEASE_CHECKLIST.md` for the complete checklist.

## Versioning

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR** (X.0.0): Breaking changes
- **MINOR** (0.X.0): New features, backwards compatible
- **PATCH** (0.0.X): Bug fixes only

Examples:
- `0.1.0` → `0.2.0`: Added remote workspace support (new feature)
- `0.2.0` → `0.2.1`: Fixed port forwarding bug
- `0.9.0` → `1.0.0`: First stable release

## Troubleshooting

### GitHub Release Failed

Check the Actions logs for errors. Common issues:
- Build failures (check Rust code)
- Permission issues (need GITHUB_TOKEN)
- Network issues (retry the workflow)

### Checksum Mismatch

If users report checksum errors:
1. Download the release binary manually
2. Compute checksum: `shasum -a 256 devdock-macos-arm64-v0.1.0`
3. Compare with `checksums.txt` in the release
4. If mismatched, delete release and re-tag

### Installer Not Working

Test the installer locally:
```bash
# Download and inspect
curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh > /tmp/install.sh
bash -x /tmp/install.sh  # Debug mode
```

## Rolling Back a Release

If critical issues are found:

```bash
# Delete the tag locally and remotely
git tag -d vX.Y.Z
git push --delete origin vX.Y.Z

# Delete the GitHub release (via web UI or gh CLI)
gh release delete vX.Y.Z

# Fix the issue, create new patch version
# Follow normal release process with vX.Y.Z+1
```

## Advanced: Testing Pre-releases

For testing before official release:

```bash
# Create a pre-release tag
git tag -a v0.2.0-rc1 -m "Release candidate 1"
git push origin v0.2.0-rc1

# CI will build it as a pre-release
# Test with:
VERSION=v0.2.0-rc1
curl -L https://github.com/askscio/devdock/releases/download/${VERSION}/devdock-macos-arm64-${VERSION} -o devdock
chmod +x devdock
./devdock --version
```

## GitHub Actions Workflows

### Release Workflow (`.github/workflows/release.yml`)

Triggered by: Tag push (`v*`)

Steps:
1. Build for all platforms (macOS ARM64, macOS x86_64, Linux x86_64)
2. Generate checksums
3. Create GitHub Release with binaries and checksums

### CI Workflow (`.github/workflows/ci.yml`)

Triggered by: Push to main, Pull requests

Steps:
1. Run tests
2. Run clippy
3. Check formatting

## Repository Structure

```
devdock/
├── install.sh              # One-line installer
├── uninstall.sh            # Uninstaller
├── CHANGELOG.md            # Release notes
├── README.md               # Installation instructions
├── .github/
│   ├── workflows/
│   │   └── release.yml     # Release automation
│   └── RELEASE_CHECKLIST.md
└── docs/
    └── publishing.md       # This file
```

## Security Considerations

- ✅ Checksums are generated for all binaries
- ✅ Installer verifies checksums before installation
- ✅ All releases are signed by GitHub Actions
- ✅ HTTPS for all downloads

## Support

For questions or issues:
- GitHub Issues: https://github.com/askscio/devdock/issues
- Internal Slack: #devdock
