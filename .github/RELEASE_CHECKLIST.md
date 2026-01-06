# Release Checklist

Use this checklist for every DevDock release.

## Pre-Release

- [ ] Update version in `Cargo.toml` (workspace.package.version)
- [ ] Update `CHANGELOG.md` with release notes
  - [ ] Document all new features
  - [ ] Document all bug fixes
  - [ ] Document breaking changes (if any)
  - [ ] Update comparison links at bottom
- [ ] Run full test suite: `cargo test --all`
- [ ] Run clippy: `cargo clippy --all-targets --all-features`
- [ ] Build and test locally: `cargo build --release && ./target/release/devdock --version`
- [ ] Test with a real profile: `./target/release/devdock start default_profile`
- [ ] Update README.md if needed
- [ ] Update documentation if needed

## Release Process

- [ ] Commit all changes: `git add . && git commit -m "Prepare for vX.Y.Z release"`
- [ ] Push to main: `git push origin main`
- [ ] Create and push tag: 
  ```bash
  git tag -a vX.Y.Z -m "Release vX.Y.Z"
  git push origin vX.Y.Z
  ```
- [ ] Wait for GitHub Actions to complete (check Actions tab)
- [ ] Verify release was created on GitHub
- [ ] Download and test binaries from GitHub release:
  - [ ] macOS ARM64
  - [ ] macOS x86_64
  - [ ] Linux x86_64
- [ ] Verify checksums match

## Verify Installation Methods

- [ ] Test the one-line installer:
  ```bash
  curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh | bash
  devdock --version
  ```
- [ ] Test manual download and verify checksums:
  ```bash
  # Download binary for your platform
  curl -L https://github.com/askscio/devdock/releases/download/vX.Y.Z/devdock-macos-arm64-vX.Y.Z -o devdock
  
  # Verify checksum
  curl -L https://github.com/askscio/devdock/releases/download/vX.Y.Z/checksums.txt
  shasum -a 256 devdock
  ```

## Post-Release

- [ ] Announce release:
  - [ ] Slack/Teams announcement
  - [ ] Email to team
  - [ ] Update internal documentation
- [ ] Monitor for issues:
  - [ ] Check GitHub Issues
  - [ ] Check Slack for user reports
- [ ] Update project board/roadmap if applicable

## Verification Commands

```bash
# Test binary works
devdock --version

# Test all major commands
devdock --help
devdock list
devdock status

# Test with real profile (optional)
devdock validate default_profile
devdock start default_profile
# ... then Ctrl+C
devdock cleanup
```

## Rollback Procedure (if needed)

If critical issues are found:

1. Delete the GitHub release and tag:
   ```bash
   git tag -d vX.Y.Z
   git push --delete origin vX.Y.Z
   ```
2. Fix issues and create new patch release
3. Announce the rollback

## Version Numbering Guide

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR** (X.0.0): Breaking changes, incompatible API changes
- **MINOR** (0.X.0): New features, backwards compatible
- **PATCH** (0.0.X): Bug fixes only, backwards compatible

Examples:
- `0.1.0` → `0.2.0`: Added hybrid watch strategy (new feature)
- `0.2.0` → `0.2.1`: Fixed database cleanup bug
- `0.9.0` → `1.0.0`: First stable release (production ready)

## Notes

- GitHub Actions automatically builds multi-platform binaries on tag push
- Release notes are auto-generated from commits, but review and edit for clarity
- Keep CHANGELOG.md updated with every release
- Always test binaries before announcing release

