# DevDock Release Process

## Setup (One-Time)

Create a GitHub App to enable CI on release PRs:

### 1. Create GitHub App
- Go to: https://github.com/organizations/askscio/settings/apps
- Click **"New GitHub App"**
- Settings:
  - Name: `DevDock Release Bot`
  - Homepage URL: `https://github.com/askscio/devdock`
  - Webhook: ❌ Uncheck "Active"
  - Permissions → Repository:
    - Contents: Read and write
    - Pull requests: Read and write
  - Install on: Only this account
- Click **"Create GitHub App"**
- Note the **App ID**

### 2. Generate Private Key
- On app settings page → **"Private keys"**
- Click **"Generate a private key"**
- Save the `.pem` file

### 3. Install App
- Click **"Install App"** → Install on `askscio/devdock`

### 4. Add Secrets
```bash
gh secret set APP_ID --body "your-app-id"
gh secret set APP_PRIVATE_KEY < path/to/key.pem
```

> **Note**: Commits and tags are automatically signed by GitHub when created via the API. No GPG key management needed! ✅

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
