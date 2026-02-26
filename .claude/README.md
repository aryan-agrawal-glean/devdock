# Testing DevDock Features with Claude

Use Claude Code to test DevDock features automatically.

## Quick Start

Just describe your feature to Claude:

```
You: "I added a --branch flag. Test it."

Claude will automatically:
- Build your code
- Test the feature
- Verify it works
- Clean up resources
```

Or invoke skills directly: `/build-and-install`, `/test-remote-devdock`, etc.

## Available Skills

| Skill | Purpose |
|-------|---------|
| `/build-and-install` | Build dev binary |
| `/test-remote-devdock` | Test remote workspace features |
| `/test-local-devdock` | Test local service features |
| `/test-sync` | Test file synchronization |
| `/verify-feature` | Run verification checks |
| `/ssh-remote` | SSH into remote workspace |
| `/check-logs` | View logs for debugging |
| `/debug-devdock` | Collect diagnostics |
| `/stop-cleanup` | Clean up test resources |
| `/quick-test` | Quick smoke test |

## Common Workflows

**Test Remote Feature:**
```
/build-and-install
/test-remote-devdock
/verify-feature
/stop-cleanup
```

**Test Sync Feature:**
```
/build-and-install
/test-remote-devdock
/test-sync
/check-logs
/stop-cleanup
```

**Debug Issue:**
```
/debug-devdock
/check-logs
/ssh-remote
```

## How It Works

Skills are generic building blocks defined in `.claude/skills.md`. Compose them to test any feature:

```
You: "Test my new --region flag"

Claude:
/build-and-install → Build binary
/test-remote-devdock → Start with --region flag
/verify-feature → Check it works
/stop-cleanup → Clean up
```

## Tips

- **Just ask Claude** - Describe what you need
- **Always build first** - Run `/build-and-install` after code changes
- **Check logs** - Use `/check-logs` to verify behavior
- **Clean up** - Use `/stop-cleanup` when done

---

**Ready to test?** Open Claude Code and describe your feature!
