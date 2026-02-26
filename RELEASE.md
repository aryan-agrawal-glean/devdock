# DevDock Release Process

## How to Release

```bash
# 1. Start release workflow
gh workflow run release.yml -f version=0.3.9

# 2. Check PR was created
gh pr list --label release

# 3. Merge the PR
gh pr merge <PR_NUMBER> --squash

# 4. Done! Tag and release are created automatically
```

## What Happens

1. Workflow creates PR with version bump in `Cargo.toml`
2. You review and merge the PR (CI checks will run)
3. Tag `v0.3.9` is created automatically
4. Release builds and publishes automatically (~10-15 min)

## Version Types

- **Patch**: `0.3.8 → 0.3.9` (bug fixes)
- **Minor**: `0.3.9 → 0.4.0` (new features)
- **Major**: `0.3.9 → 1.0.0` (breaking changes)
