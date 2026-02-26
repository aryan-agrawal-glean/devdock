# DevDock Testing Skills

Generic, composable skills for testing any DevDock feature.

---

## /build-and-install

Build and install DevDock from source for testing new features.

### Steps

1. Build and install the dev binary:
   ```bash
   cargo install --path crates/devdock-cli --debug --force
   ```

2. Verify installation:
   ```bash
   devdock --version
   ```

3. Display the installed version and binary location:
   ```bash
   which devdock
   ```

### Notes

- Uses `--debug` flag for faster compilation during development
- Uses `--force` to overwrite existing installation
- The binary will be installed to `~/.cargo/bin/devdock`

---

## /test-remote-devdock

Test remote DevDock features including workspace creation, branch selection, and service startup.

### Prerequisites

- Dev binary installed (use `/build-and-install` skill)
- Located in `~/workspace/scio` directory
- Valid profile name

### Steps

1. Navigate to scio workspace:
   ```bash
   cd ~/workspace/scio
   ```

2. Start remote DevDock with the specified profile and feature flags:
   ```bash
   devdock start <profile> -m remote [--branch <branch-name>] [--workspace_name <name>] [--region <region>] [--auto_forward] [--verbose]
   ```

3. Wait for the workspace to be provisioned (this takes 2-3 minutes)

4. Once started, verify services are running by checking Tilt status:
   ```bash
   coder ssh <workspace-name>
   # Then inside the remote workspace:
   tilt get all
   ```

5. Monitor specific service logs:
   ```bash
   # Inside the remote workspace via coder ssh:
   tilt logs -f <resource-name>
   # Or check docker logs directly:
   docker logs <container-name> -f
   ```

6. For branch flag testing, verify:
   - Branch exists on remote: `git branch -r | grep <branch-name>`
   - Workspace is on correct branch: `git rev-parse --abbrev-ref HEAD`
   - File sync synced all files: `ls -la` and compare with local

7. Check sync status:
   ```bash
   devdock sync status
   ```

### Common Test Scenarios

#### Test --branch flag with existing branch
```bash
devdock start <profile> -m remote --branch feature-xyz
```

#### Test --branch flag with new local branch
```bash
git checkout -b test-branch
devdock start <profile> -m remote --branch test-branch
# Should auto-push the branch to remote
```

#### Test --branch different from current branch
```bash
git checkout main
devdock start <profile> -m remote --branch develop
# Should warn about mismatch
```

#### Test workspace name override
```bash
devdock start <profile> -m remote --workspace_name my-test-ws
```

### Verification Checklist

- [ ] Workspace created successfully
- [ ] Remote on correct branch (if --branch specified)
- [ ] All services started in Tilt
- [ ] Port forwarding working (Tilt UI accessible)
- [ ] File sync active and syncing files
- [ ] Can SSH into workspace: `coder ssh <workspace-name>`

### Cleanup

Stop and remove the remote workspace when done:
```bash
devdock stop <instance-id>
```

---

## /test-local-devdock

Test local DevDock features including service startup, port management, and Tilt integration.

### Prerequisites

- Dev binary installed (use `/build-and-install` skill)
- Located in `~/workspace/scio` directory
- Valid profile name

### Steps

1. Navigate to scio workspace:
   ```bash
   cd ~/workspace/scio
   ```

2. Start local DevDock with the specified profile:
   ```bash
   devdock start <profile> [--services <service1,service2>] [--monitor] [--verbose]
   ```

3. Wait for services to start (this varies by profile)

4. Verify Tilt is running:
   ```bash
   # Tilt UI should be accessible at http://localhost:10350
   curl -s http://localhost:10350 | head -5
   ```

5. Check service status via Tilt:
   ```bash
   tilt get all
   ```

6. Monitor specific service logs:
   ```bash
   tilt logs -f <resource-name>
   # Or check docker logs directly:
   docker logs <container-name> -f
   ```

7. Check DevDock instance status:
   ```bash
   devdock status
   ```

### Common Test Scenarios

#### Test specific services only
```bash
devdock start <profile> --services api,web
```

#### Test with monitoring (wait for healthy)
```bash
devdock start <profile> --monitor --monitor_timeout 600
```

#### Test verbose mode
```bash
devdock start <profile> --verbose
```

#### Test port allocation
```bash
devdock start <profile>
# Check allocated ports:
devdock status
```

### Verification Checklist

- [ ] Tilt started successfully
- [ ] All requested services started
- [ ] Services are healthy (if using --monitor)
- [ ] Port forwarding working
- [ ] Can access services locally
- [ ] No port conflicts
- [ ] Docker containers running: `docker ps`

### Check Service Health

#### Via Docker
```bash
docker ps --format "table {{.Names}}\t{{.Status}}"
```

#### Via Tilt
```bash
tilt get resources
```

#### Via Logs
```bash
# Check for errors in service logs
tilt logs <service-name> | grep -i error
```

### Cleanup

Stop the local DevDock instance when done:
```bash
devdock stop <instance-id>
# Or stop all:
devdock stop --all
```

### Common Issues

#### Port conflicts
If services fail to start due to port conflicts:
```bash
# Find what's using the port:
lsof -i :<port-number>
# Kill the process or use different port base:
devdock start <profile> --port_base 20000
```

#### Services stuck starting
```bash
# Check Tilt logs:
tilt logs -f <service-name>
# Check docker logs:
docker logs <container-name>
```

#### Tilt not accessible
```bash
# Verify Tilt is running:
ps aux | grep tilt
# Check Tilt is on correct port:
netstat -an | grep 10350
```

---

## /test-sync

Test DevDock file synchronization between local and remote workspaces.

### Prerequisites

- Remote DevDock instance running
- Dev binary installed (use `/build-and-install` skill)

### Steps

#### 1. Check Sync Status

```bash
devdock sync status
```

Expected output should show:
- Sync daemon running
- Recent successful syncs
- Files synced count
- Uptime

#### 2. Test Automatic File Sync

1. Create a test file locally:
   ```bash
   cd ~/workspace/scio
   echo "test sync" > test-sync-file.txt
   ```

2. Wait a few seconds for sync to trigger (debounce is ~500ms)

3. Verify file appears on remote:
   ```bash
   coder ssh <workspace-name>
   # Inside remote:
   cat test-sync-file.txt
   ```

4. Modify the file locally:
   ```bash
   echo "updated content" >> test-sync-file.txt
   ```

5. Verify changes synced to remote:
   ```bash
   coder ssh <workspace-name>
   cat test-sync-file.txt
   ```

#### 3. Test Git-Tracked File Sync

1. Modify a tracked file:
   ```bash
   cd ~/workspace/scio
   echo "// test change" >> data/some-file.go
   ```

2. Check sync logs:
   ```bash
   devdock sync history --tail 10
   ```

3. Verify on remote:
   ```bash
   coder ssh <workspace-name>
   git diff
   ```

#### 4. Test Untracked File Sync

1. Create an untracked file in a monitored directory:
   ```bash
   touch python_scio/new_untracked_file.py
   echo "# new file" > python_scio/new_untracked_file.py
   ```

2. Verify it syncs (should sync if in monitored dirs and matches patterns)

3. Check on remote:
   ```bash
   coder ssh <workspace-name>
   ls python_scio/new_untracked_file.py
   ```

#### 5. Test Sync Modes

##### Git-tracked mode (default for manual sync)
```bash
devdock sync start
```

##### Full mode (sync everything)
```bash
devdock sync start --full
```

##### Branch mode (all git-tracked + untracked)
```bash
# Automatically enabled when using --branch flag with remote start
devdock start <profile> -m remote --branch <branch>
```

#### 6. Monitor Sync Health

1. Check sync errors:
   ```bash
   devdock sync errors --tail 20
   ```

2. View detailed sync history:
   ```bash
   devdock sync history --tail 50
   ```

3. Check sync daemon logs:
   ```bash
   cat ~/.devdock/sync/<devdock-id>.log
   ```

#### 7. Test Sync Stop/Restart

1. Stop sync:
   ```bash
   devdock sync stop
   ```

2. Verify sync stopped:
   ```bash
   devdock sync status
   # Should show no active sync
   ```

3. Restart sync:
   ```bash
   devdock sync start
   ```

### Verification Checklist

- [ ] Sync daemon is running
- [ ] New files sync automatically
- [ ] Modified files sync automatically
- [ ] Untracked files in monitored dirs sync
- [ ] Git-tracked files sync
- [ ] Sync status shows accurate info
- [ ] No errors in sync logs
- [ ] Can stop and restart sync successfully

### Monitored Directories

Files are synced from these directories:
- `data/`
- `go/`
- `python_scio/`
- `typescript/`
- `java/`
- `com/askscio/proto/`
- `config/`
- `javatests/`
- `openapi/`
- `devdock/`

### Sync File Patterns

Only files with these extensions sync:
- `.bazel`, `.go`, `.py`, `.ts`, `.tsx`, `.java`, `.proto`
- `.ini`, `.json`, `.yaml`, `.yml`, `.sh`, `.sql`
- `.gradle`, `.properties`

Plus special files:
- `pom.xml`, `Makefile`, `package.json`, `pnpm-lock.yaml`

### Cleanup

Remove test files:
```bash
rm -f test-sync-file.txt
git restore data/some-file.go
rm -f python_scio/new_untracked_file.py
```

---

## /verify-feature

Generic verification steps for any DevDock feature.

### Basic Verification

#### 1. Check DevDock Status
```bash
devdock status
devdock --version
```

#### 2. Check Instance Running
```bash
devdock status
# Look for your instance ID and status
```

#### 3. Check Sync Active
```bash
devdock sync status
```

### Remote Feature Verification

#### 1. Verify Workspace Created
```bash
coder list | grep <workspace-name>
```

#### 2. Verify Can SSH
```bash
coder ssh <workspace-name> -- echo "Connected successfully"
```

#### 3. Verify Services Running
```bash
# Check Tilt
coder ssh <workspace-name> -- tilt get all

# Check Docker
coder ssh <workspace-name> -- docker ps
```

#### 4. Verify Port Forwarding
```bash
# Check Tilt UI accessible
curl -s http://localhost:10350 | head -5

# Or open in browser
open http://localhost:10350
```

#### 5. Verify File Sync Working
```bash
# Create test file
echo "test" > /tmp/test-verify-$$.txt

# Move to synced directory
mv /tmp/test-verify-$$.txt ~/workspace/scio/data/

# Wait 2 seconds
sleep 2

# Check on remote
coder ssh <workspace-name> -- cat data/test-verify-$$.txt

# Cleanup
rm ~/workspace/scio/data/test-verify-$$.txt
```

### Local Feature Verification

#### 1. Verify Tilt Running
```bash
curl -s http://localhost:10350 | head -5
# Or
tilt get all
```

#### 2. Verify Services
```bash
tilt get all
docker ps
```

#### 3. Verify Logs
```bash
tilt logs <service-name> | tail -20
```

### Branch-Specific Verification

#### 1. Check Local Branch
```bash
cd ~/workspace/scio
git rev-parse --abbrev-ref HEAD
```

#### 2. Check Remote Branch
```bash
coder ssh <workspace-name> -- git rev-parse --abbrev-ref HEAD
```

#### 3. Check Branch Exists
```bash
# Locally
git branch -a | grep <branch-name>

# On remote
git ls-remote origin refs/heads/<branch-name>
```

#### 4. Verify Sync Mode
```bash
# Check logs for sync mode
cat ~/.devdock/sync/<devdock-id>.log | grep "Performing initial sync"
# Should show: "mode: git-tracked" or "mode: branch" or "mode: full"
```

### Sync-Specific Verification

#### 1. Check Sync Status
```bash
devdock sync status
```

#### 2. Check Recent Syncs
```bash
devdock sync history --tail 10
```

#### 3. Check Sync Errors
```bash
devdock sync errors --tail 10
```

#### 4. Verify File Count
```bash
# Check how many files synced
cat ~/.devdock/sync/<devdock-id>.log | grep "Found.*files to sync"
```

### Generic Checklist

- [ ] Instance created and running
- [ ] Can SSH to remote (if remote)
- [ ] Services started successfully
- [ ] Port forwarding working (if applicable)
- [ ] File sync active
- [ ] No errors in logs
- [ ] Feature-specific behavior works as expected

---

## /ssh-remote

SSH into a remote DevDock workspace to inspect, verify, or debug.

### Usage

```bash
# List available workspaces
coder list

# SSH into specific workspace
coder ssh <workspace-name>

# SSH and run a command
coder ssh <workspace-name> -- <command>

# SSH and check branch
coder ssh <workspace-name> -- git rev-parse --abbrev-ref HEAD

# SSH and check git status
coder ssh <workspace-name> -- git status

# SSH and list files
coder ssh <workspace-name> -- ls -la

# SSH and check Tilt
coder ssh <workspace-name> -- tilt get all
```

### Common Verification Commands

#### Check Git Branch
```bash
coder ssh <workspace-name> -- git rev-parse --abbrev-ref HEAD
```

#### Check Files Synced
```bash
coder ssh <workspace-name> -- ls -la <file-path>
```

#### Check Service Status
```bash
coder ssh <workspace-name> -- tilt get all
coder ssh <workspace-name> -- docker ps
```

#### Check Logs
```bash
coder ssh <workspace-name> -- tilt logs <service-name>
coder ssh <workspace-name> -- docker logs <container-name>
```

#### Check Git Diff
```bash
coder ssh <workspace-name> -- git diff
coder ssh <workspace-name> -- git status
```

### Interactive Session

For interactive debugging:
```bash
coder ssh <workspace-name>
# Now you're in the remote workspace
# Can run any commands: git, tilt, docker, etc.
```

---

## /check-logs

View logs for debugging and verification.

### Sync Logs

```bash
# List sync log files
ls -lth ~/.devdock/sync/*.log | head -5

# View latest sync log
ls -t ~/.devdock/sync/*.log | head -1 | xargs tail -50

# Check sync mode used
cat ~/.devdock/sync/<devdock-id>.log | grep "Performing initial sync"

# Check files synced
cat ~/.devdock/sync/<devdock-id>.log | grep "Found.*files to sync"

# Check sync errors
devdock sync errors --tail 20

# Check sync history
devdock sync history --tail 20
```

### Operation Logs

```bash
# List operation logs
ls -lth ~/.devdock/operations/*.log | head -5

# View latest operation log
ls -t ~/.devdock/operations/*.log | head -1 | xargs cat

# Search for errors in operations
ls -t ~/.devdock/operations/*.log | head -1 | xargs grep -i error

# Check specific instance log
cat ~/.devdock/operations/<instance-id>.log
```

### Tilt Logs

```bash
# Local Tilt logs
tilt logs <service-name>
tilt logs -f <service-name>  # Follow

# Remote Tilt logs
coder ssh <workspace-name> -- tilt logs <service-name>
coder ssh <workspace-name> -- tilt logs -f <service-name>
```

### Docker Logs

```bash
# Local docker logs
docker logs <container-name>
docker logs -f <container-name>  # Follow
docker logs --tail 100 <container-name>

# Remote docker logs
coder ssh <workspace-name> -- docker logs <container-name>
```

### Coder Logs

```bash
# Check coder workspace logs
coder list --output json | jq '.[] | {name, status}'

# Check specific workspace
coder show <workspace-name>
```

### Search Logs

```bash
# Search for specific text in sync logs
grep -i "branch" ~/.devdock/sync/<devdock-id>.log

# Search for errors
grep -i "error\|fail" ~/.devdock/sync/<devdock-id>.log

# Search for warnings
grep -i "warn" ~/.devdock/sync/<devdock-id>.log
```

### Log Locations

```bash
# Sync logs
~/.devdock/sync/<devdock-id>.log

# Operation logs
~/.devdock/operations/<instance-id>.log

# Sync stats
~/.devdock/sync/<devdock-id>.stats.json

# Sync history
~/.devdock/sync/<devdock-id>.history.jsonl

# Sync errors
~/.devdock/sync/<devdock-id>.errors.jsonl
```

---

## /debug-devdock

Debug common DevDock issues and collect diagnostic information.

### Common Issues & Solutions

#### Issue 1: Workspace Creation Fails

**Symptoms:**
- Error during `devdock start -m remote`
- Workspace creation timeout

**Debug:**
```bash
# Check gcloud auth
gcloud auth print-access-token

# Check coder connectivity
coder list

# Verify template exists
coder templates list | grep devdock-glean

# Check workspace status
coder list | grep <workspace-name>
```

**Logs to check:**
```bash
# DevDock instance logs
cat ~/.devdock/instances.db

# Coder logs
coder list --output json
```

---

#### Issue 2: File Sync Not Working

**Symptoms:**
- Files not appearing on remote
- Sync daemon crashed

**Debug:**
```bash
# Check sync status
devdock sync status

# Check sync daemon process
ps aux | grep devdock | grep sync_daemon

# Check recent errors
devdock sync errors --tail 20

# View sync history
devdock sync history --tail 50

# Check sync daemon logs
cat ~/.devdock/sync/<devdock-id>.log
```

**Common fixes:**
```bash
# Restart sync
devdock sync stop
devdock sync start

# Or use full sync mode
devdock sync start --full
```

---

#### Issue 3: Services Not Starting

**Symptoms:**
- Tilt services stuck in pending/error state
- Docker containers not running

**Debug:**
```bash
# Check Tilt status
tilt get all

# Check specific service logs
tilt logs <service-name>

# Check docker containers
docker ps -a

# Check docker logs
docker logs <container-name>

# Check resource usage
docker stats --no-stream
```

**Common fixes:**
```bash
# Restart specific service in Tilt UI or:
tilt up <service-name>

# Check for port conflicts
lsof -i :<port-number>

# Check docker daemon
docker info
```

---

#### Issue 4: Port Forwarding Issues

**Symptoms:**
- Can't access Tilt UI
- Services not accessible locally

**Debug:**
```bash
# Check port forwarding processes
ps aux | grep "port-forward\|ssh.*coder"

# Check what's listening on expected ports
netstat -an | grep LISTEN | grep <port>

# For remote, check coder ssh
coder ssh <workspace-name> --no-tty ps aux | grep tilt
```

**Common fixes:**
```bash
# Kill stale port forwards
pkill -f "coder.*port-forward"

# Restart DevDock instance
devdock stop <instance-id>
devdock start <profile> -m remote
```

---

#### Issue 5: Branch Flag Not Working

**Symptoms:**
- Remote not on expected branch
- Auto-push failed
- Branch validation error

**Debug:**
```bash
# Check branch exists locally
git branch -a | grep <branch-name>

# Check branch exists on remote
git ls-remote origin refs/heads/<branch-name>

# Check current branch
git rev-parse --abbrev-ref HEAD

# Check remote workspace branch
coder ssh <workspace-name>
git rev-parse --abbrev-ref HEAD

# Check sync mode in logs
cat ~/.devdock/sync/<devdock-id>.log | grep "Performing initial sync"
```

**Verification:**
```bash
# Test branch validation
git rev-parse --verify <branch-name>

# Test push capability
git push --dry-run -u origin <branch-name>
```

---

### Collect Full Diagnostic Info

Run this to collect all diagnostic information:

```bash
#!/bin/bash
echo "=== DevDock Version ==="
devdock --version

echo -e "\n=== Git Status ==="
cd ~/workspace/scio
git status
git branch -a

echo -e "\n=== DevDock Instances ==="
devdock status

echo -e "\n=== Coder Workspaces ==="
coder list

echo -e "\n=== Sync Status ==="
devdock sync status

echo -e "\n=== Recent Sync Errors ==="
devdock sync errors --tail 10

echo -e "\n=== Docker Containers ==="
docker ps -a

echo -e "\n=== Port Listeners ==="
netstat -an | grep LISTEN | grep -E "10350|20000"

echo -e "\n=== Sync Daemon Processes ==="
ps aux | grep devdock | grep -v grep

echo -e "\n=== Recent Sync Logs ==="
LATEST_LOG=$(ls -t ~/.devdock/sync/*.log 2>/dev/null | head -1)
if [ -n "$LATEST_LOG" ]; then
  tail -50 "$LATEST_LOG"
fi

echo -e "\n=== Tilt Status (if accessible) ==="
curl -s http://localhost:10350/api/v2/resources 2>/dev/null | head -20 || echo "Tilt not accessible"
```

Save this as `~/devdock-diagnostics.sh` and run:
```bash
chmod +x ~/devdock-diagnostics.sh
./devdock-diagnostics.sh > devdock-debug.txt 2>&1
cat devdock-debug.txt
```

---

### Performance Debugging

#### Check Startup Time
```bash
# Time the startup
time devdock start <profile> -m remote

# Check which phase is slow:
# - gcloud auth check
# - Coder workspace creation (1-2 min expected)
# - Remote devdock start
# - Tilt startup
# - File sync
```

#### Check Sync Performance
```bash
# Check file count being synced
cat ~/.devdock/sync/<devdock-id>.log | grep "Found.*files to sync"

# Check sync batch size and time
devdock sync history --tail 20

# Monitor sync in real-time
tail -f ~/.devdock/sync/<devdock-id>.log
```

#### Check Resource Usage
```bash
# Local machine
top -l 1 | grep -E "CPU|PhysMem"

# Docker resources
docker stats --no-stream

# On remote workspace
coder ssh <workspace-name>
top -bn1 | head -20
```

---

### Log Locations

```bash
# Sync daemon logs
~/.devdock/sync/<devdock-id>.log

# Sync stats
~/.devdock/sync/<devdock-id>.stats.json

# Sync history
~/.devdock/sync/<devdock-id>.history.jsonl

# Sync errors
~/.devdock/sync/<devdock-id>.errors.jsonl

# Instance database
~/.devdock/instances.db

# Operation logs
~/.devdock/operations/<instance-id>.log
```

---

### Cleanup Stale Resources

```bash
# Stop all instances
devdock stop --all

# Kill stale sync daemons
pkill -f "__sync_daemon__"

# Clean up stale coder workspaces
coder list | grep -v "Running\|Stopped" | awk '{print $1}' | xargs -I {} coder delete {}

# Clean up docker containers
docker ps -a --filter "status=exited" --filter "status=created" -q | xargs docker rm

# Clean up port forwards
pkill -f "coder.*port-forward"
```

---

## /stop-cleanup

Stop DevDock instances and clean up resources.

### Stop Instance

#### Stop Specific Instance
```bash
# Get instance ID
devdock status

# Stop by ID
devdock stop <instance-id>
```

#### Stop All Instances
```bash
devdock stop --all
```

### Cleanup Sync

#### Stop Sync
```bash
devdock sync stop
```

#### Check Sync Stopped
```bash
devdock sync status
# Should show no active sync
```

#### Kill Stale Sync Daemons
```bash
pkill -f "__sync_daemon__"
```

### Cleanup Coder Workspaces

#### List Workspaces
```bash
coder list
```

#### Delete Specific Workspace
```bash
coder delete <workspace-name>
```

#### Delete All Stopped Workspaces
```bash
coder list | grep Stopped | awk '{print $1}' | xargs -I {} coder delete {}
```

### Cleanup Docker

#### Stop All Containers
```bash
docker stop $(docker ps -q)
```

#### Remove Stopped Containers
```bash
docker ps -a --filter "status=exited" -q | xargs docker rm
```

#### Clean Up Docker Resources
```bash
docker system prune -f
```

### Cleanup Port Forwards

#### Kill Port Forward Processes
```bash
pkill -f "coder.*port-forward"
pkill -f "port-forward"
```

#### Check What's Listening on Ports
```bash
# Check common DevDock ports
lsof -i :10350  # Tilt UI
lsof -i :20000  # Common port base
```

### Cleanup Test Files

#### Remove Test Files from Scio
```bash
cd ~/workspace/scio

# Remove test files
rm -f test-*.txt
rm -f data/test-*.txt
rm -f python_scio/test-*.py

# Restore git changes if needed
git restore .
git clean -fd  # Remove untracked files
```

### Full Cleanup

Run all cleanup steps:
```bash
# Stop all instances
devdock stop --all

# Stop sync
devdock sync stop

# Kill stale processes
pkill -f "__sync_daemon__"
pkill -f "coder.*port-forward"

# Clean docker
docker stop $(docker ps -q) 2>/dev/null
docker ps -a --filter "status=exited" -q | xargs docker rm 2>/dev/null

# Clean test files
cd ~/workspace/scio
git restore .
git clean -fd

echo "✅ Cleanup complete"
```

### Verify Cleanup

```bash
# Check no instances running
devdock status

# Check no sync running
devdock sync status

# Check no docker containers
docker ps

# Check no port forwards
lsof -i :10350
lsof -i :20000

echo "✅ All clean"
```

---

## /quick-test

Quickly test a DevDock feature with common defaults.

### Usage

For testing remote features:
```bash
# 1. Build and install
cargo install --path crates/devdock-cli --debug --force

# 2. Go to scio
cd ~/workspace/scio

# 3. Start with feature flag
devdock start <profile> -m remote <feature-flags>

# Examples:
devdock start default_profile -m remote --branch main
devdock start default_profile -m remote --verbose
devdock start default_profile -m remote --auto_forward
```

For testing local features:
```bash
# 1. Build and install
cargo install --path crates/devdock-cli --debug --force

# 2. Go to scio
cd ~/workspace/scio

# 3. Start locally
devdock start <profile>
```

For testing sync:
```bash
# Check status
devdock sync status

# Create test file
echo "test" > test-sync.txt

# Wait 2 seconds

# Verify on remote
coder ssh <workspace-name>
cat test-sync.txt
```

### Quick Verification Commands

```bash
# Check version
devdock --version

# Check instances
devdock status

# Check sync
devdock sync status

# Check logs
cat ~/.devdock/sync/<devdock-id>.log | tail -50

# SSH to remote
coder ssh <workspace-name>

# Check Tilt (on remote or local)
tilt get all
```
