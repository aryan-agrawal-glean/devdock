# DevDock OSS Testing Plan

## Overview

This document covers the full E2E test plan for the DevDock OSS refactor.
The goal is zero regression across all profiles (GCP, AWS, pipelines, tests) that
worked with the previous binary.

---

## Feature Coverage Matrix

| Feature | Profiles to Test | Status |
|---------|-----------------|--------|
| GCP platform (google) | qe, admin, pipelines | ✅ qe done |
| AWS platform (aws) | aws_profile | ❌ pending |
| SQL local (fast mode) | qe, ci_smoke_test | ✅ qe done |
| SQL local (latest mode) | pipelines | ❌ pending |
| SQL full_instance_export | examples/ | ❌ pending |
| OpenSearch remote | qe, aws_profile | ❌ pending (qe OS skipped) |
| OpenSearch local | qa_tests | ❌ pending |
| Nginx + pubsub + memcached | qe | ✅ done |
| Redis | qe (has redis) | ✅ done |
| Bastion tunnel | aws_profile, admin | ❌ pending |
| RabbitMQ | aws_profile | ❌ pending |
| Pipelines section | pipelines | ❌ pending |
| Test suite section | deployment_wizard_tests | ❌ pending |
| Remote services | admin | ❌ pending |
| hot_reload: disable | aws_profile (crawler) | ❌ pending |
| ini_overrides | examples/ | ❌ pending |
| service_hooks (pre_scripts) | qa_tests | ❌ pending |
| --services filter | any | ❌ pending |
| dry_run mode | any | ❌ pending |
| devdock stop clean | all | ✅ qe done |
| devdock status | any running | ❌ pending |
| devdock report | manual | ❌ pending |
| devdock infra sql setup | standalone | ❌ pending |
| devdock infra opensearch copy | standalone | ❌ pending |
| bigquery-sync telemetry | any (silent) | ❌ pending |
| slack-notifier | any after_ready | ❌ pending |

---

## Known Bug: Platform Override (Fix First)

**Problem**: `CLOUD_PLATFORM` env var is set from `devdock.toml [cloud] platform = "google"`,
but profile yaml can specify `platform: aws`. The profile's platform is never used to override
`CLOUD_PLATFORM`. This means:

- `temp_profile.yaml` gets `platform: google` even for AWS profiles
- SQL Tiltfile line checks `profile['platform'] == 'aws'` → fails → wrong S3 code path
- `config-fetcher` plugin gets `platform: google` init config → tries GCS instead of S3
- `platform-auth` plugin validates GCP credentials instead of AWS

**Fix**: In `start.rs`, let `profile.platform` override `project_config.cloud.platform`:

```rust
// Allow profile.yaml to override devdock.toml platform
let platform_str = if !matches!(profile.platform, Platform::Google)
    && !matches!(profile.platform, Platform::None)
{
    format!("{}", profile.platform).to_uppercase() // use profile platform
} else {
    project_config.cloud.platform.to_uppercase()   // use devdock.toml platform
};
std::env::set_var("CLOUD_PLATFORM", &platform_str);
```

Also update plugin init config to use the resolved platform, not always `project_config.cloud.platform`.

---

## Test 1: GCP QE Profile (Baseline) ✅ DONE

**Profile**: `devdock/profiles/qe.yaml`
**Platform**: GCP (salessavvy-test)
**Result**: 11/11 resources healthy in ~3 minutes

**What was validated**:
- Lifecycle state machine (all 12 states)
- Plugin hooks (platform-auth, config-fetcher, config-writer, sql-hydrator)
- SQL fast mode with table copy (keyval:KeyVal)
- OpenSearch remote (hydration skipped if OS is remote)
- Nginx, pubsub, memcached, redis all healthy
- devdock stop clean

---

## Test 2: AWS Profile

**Profile**: `devdock/profiles/aws_profile.yaml`
**Platform**: AWS (aws-salessavvy-test)
**Credentials**: Set these before running:
```bash
export AWS_ACCESS_KEY_ID="ASIAS6EW7GWGQ5TPOWWQ"
export AWS_SECRET_ACCESS_KEY="252btF4oRNS4PLGbOmFfkhcgEgjyME02j7oswGkq"
export AWS_SESSION_TOKEN="IQoJb3JpZ2luX2VjEB0a..."
export AWS_REGION=us-east-1
```

**Pre-flight checks**:
```bash
aws sts get-caller-identity   # should show aws-salessavvy-test account
aws s3 ls                     # should work
```

**Pre-requisites (fix first)**:
1. Fix platform override bug (see above)
2. Update `devdock-glean-plugins/config-fetcher` to correctly route to S3 when platform=aws
3. Verify `devdock-glean-plugins/sql-hydrator` uses S3 export for aws
4. Verify `platform-auth` plugin handles AWS (currently only does GCP gcloud check)

**Run**:
```bash
cd ~/workspace/scio
devdock start aws_profile
```

**Expected resources** (from aws_profile.yaml):
| Resource | Type | Expected |
|----------|------|----------|
| sql | local | ok (S3 export, fast mode) |
| opensearch | remote | ok (port-forward) |
| nginx | local | ok |
| pubsub | local | ok |
| memcached | local | ok |
| bastion | local | ok |
| rabbitmq | local | ok |
| qe | local | ok |
| crawler | local | ok |
| docbuilder | local | ok |

**Success criteria**: 10/10 resources healthy

**Likely failure points & fixes**:
1. `platform: google` in temp_profile.yaml → Apply platform override fix
2. config-fetcher GCS failure → Check S3 bucket exists for aws-salessavvy-test
3. sql-hydrator S3 export missing → Check aws_profile sql fast export config
4. RabbitMQ Tiltfile path issues → Check `devdock/infra/rabbitmq/Tiltfile` paths

---

## Test 3: GCP Pipelines Profile

**Profile**: `devdock/profiles/pipelines.yaml`
**Platform**: GCP (salessavvy-test)

**Pre-flight**:
```bash
gcloud config set project salessavvy-test
gcloud auth application-default login
```

**Run**:
```bash
cd ~/workspace/scio
devdock start pipelines
```

**Expected resources** (from pipelines.yaml):
| Resource | Type | Expected |
|----------|------|----------|
| sql | local | ok (latest mode, 3 table groups) |
| opensearch | remote | ok |
| pubsub | local | ok |
| entitybuilder | pipeline | ok |
| generate_docs_dump | pipeline | ok |
| group_utilization_stats | pipeline | ok (with docsDumpPattern param) |

**Success criteria**: All pipeline resources healthy, pipeline jobs run to completion

**Likely failure points**:
1. `pipelines.Tiltfile` not loading → Check `lib/pipelines.Tiltfile` is extracted correctly
2. Pipeline params (`docsDumpPattern`) not passed → Check temp_profile.yaml pipelines section
3. SQL latest mode slow → Allow more time

**Check pipelines section in temp_profile.yaml**:
```bash
cat ~/workspace/scio/devdock/runtime_generated/temp_profile.yaml | grep -A20 "pipelines:"
```

---

## Test 4: GCP Integration Tests (Test Suite)

**Profile**: `devdock/profiles/integration_tests/deployment_wizard_tests.yaml`
**Platform**: GCP (glean-apps-canary)

**Pre-flight**:
```bash
gcloud config set project glean-apps-canary
```

**Run**:
```bash
cd ~/workspace/scio
devdock start integration_tests/deployment_wizard_tests
```

**Expected resources**:
| Resource | Type | Expected |
|----------|------|----------|
| sql | local | ok (4 table groups) |
| pubsub | local | ok |
| deployment_wizard | local | ok |
| internal_admin | local | ok |
| test_deployment_wizard_controllers | test | completed |

**Success criteria**: Test suite resources appear, tests run and pass/fail correctly

**Likely failure points**:
1. `test_suite.Tiltfile` not loading → Check `lib/test_suite.Tiltfile` is extracted
2. `DW_SET_TEST_ADMIN_HEADER: True` env not passed → Check temp_profile.yaml services section
3. Test dependencies not tracked → Check test_suite section in temp_profile.yaml

---

## Test 5: GCP QE with --services Filter

**Profile**: `devdock/profiles/qe.yaml`
**Purpose**: Test that `--services` filter only starts specified services

**Run**:
```bash
cd ~/workspace/scio
devdock start qe --services qe
```

**Expected**: Only `qe` service active; all infra still starts; other services in profile but inactive

**Check**:
```bash
cat ~/workspace/scio/devdock/runtime_generated/temp_profile.yaml | grep "active:"
# qe: active: "true", all others: active: "false"
```

---

## Test 6: Dry-run Mode

**Purpose**: Validate config generation without starting Tilt

**Run**:
```bash
cd ~/workspace/scio
devdock start qe --dry-run
```

**Expected output**:
```
{"profile": "qe", "bootstrap_time_ms": <N>}
Dry-run completed in Nms
```

**Check generated files**:
```bash
cat ~/workspace/scio/devdock/runtime_generated/temp_profile.yaml
cat ~/workspace/scio/devdock/devdock/devdock.ini | head -30
ls ~/workspace/scio/devdock/runtime_generated/core/lib/
```

---

## Test 7: devdock infra Commands

**Purpose**: Validate standalone CLI infra commands

**SQL setup**:
```bash
cd ~/workspace/scio
# First start a session so SQL is running
devdock start qe &
sleep 60  # wait for SQL to be healthy
devdock infra sql setup
```

**OpenSearch copy** (needs OS running):
```bash
devdock infra opensearch copy
```

**Expected**: Both commands succeed without Tiltfile involvement

---

## Test 8: devdock stop Regression

**Purpose**: Verify clean stop removes all generated artifacts

**Run**:
```bash
cd ~/workspace/scio
devdock start qe
# Wait for healthy
devdock stop
```

**Verify cleanup**:
```bash
# These should NOT exist after stop:
ls ~/workspace/scio/devdock/runtime_generated/   # should be gone or mostly empty
ls ~/workspace/scio/devdock/devdock.ini           # should NOT exist
ls ~/workspace/scio/devdock/services/*/Dockerfile 2>/dev/null  # should be gone
ls ~/workspace/scio/devdock/docker-compose.yaml  # should be gone

# These SHOULD still exist:
ls ~/workspace/scio/devdock/config/devdock.ini    # config dir untouched
ls ~/workspace/scio/.devdock/devdock.toml         # project config untouched
```

---

## Test 9: devdock status + report

**Purpose**: Validate MCP-adjacent status commands

```bash
cd ~/workspace/scio
devdock start qe &
sleep 120
devdock status           # should show running instance
devdock report           # should trigger on_report hook → jira-reporter
```

---

## Test Execution Order

```
Priority 1 (fix bugs first):
  [ ] Fix platform override bug in start.rs
  [ ] Fix plugin init to use resolved platform

Priority 2 (run in order):
  [✅] Test 1: GCP QE (baseline) — already done
  [ ]  Test 6: Dry-run (fast, validates config gen)
  [ ]  Test 8: Stop regression (validates cleanup)
  [ ]  Test 5: --services filter (validates service activation)
  [ ]  Test 3: GCP Pipelines (validates pipelines.Tiltfile)
  [ ]  Test 4: Integration Tests (validates test_suite.Tiltfile)
  [ ]  Test 7: infra commands (validates standalone plugin calls)
  [ ]  Test 2: AWS Profile (needs platform fix + credentials)

Priority 3 (stretch):
  [ ]  Test 9: status + report commands
  [ ]  GCP Admin (bastion + remote services) — partially done
  [ ]  multi_project profile (create one if not exists)
```

---

## How to Fix Issues

### Config generation issues
```bash
# Always check temp_profile.yaml first
cat ~/workspace/scio/devdock/runtime_generated/temp_profile.yaml

# Check devdock.ini was generated
cat ~/workspace/scio/devdock/devdock/devdock.ini | head -50

# Check plugin output (stderr)
# Plugins write to stderr, Tilt captures it
# Check Tilt UI -> devdock logs or:
devdock start <profile> 2>&1 | grep "\[config-\|platform-\|sql-\|opensearch-"
```

### Tiltfile issues
```bash
# Check generated Tiltfile
cat ~/workspace/scio/devdock/runtime_generated/Tiltfile

# Check Starlark libs were extracted
ls ~/workspace/scio/devdock/runtime_generated/core/lib/
```

### SQL hydrator issues
```bash
# Run standalone
devdock infra sql setup

# Check fast_export_dbs.yaml is bundled
# In sql-hydrator binary
```

### Platform issues (AWS)
```bash
# Verify CLOUD_PLATFORM is set correctly
env | grep CLOUD_PLATFORM   # should be AWS for aws_profile

# After platform fix, check temp_profile.yaml
grep "platform:" ~/workspace/scio/devdock/runtime_generated/temp_profile.yaml
# Should say: platform: aws
```

### Plugin issues
```bash
# Test plugin protocol manually
echo '{"method":"initialize","config":{"platform":"aws","project":"aws-salessavvy-test"}}' \
  | devdock-plugin-config-fetcher

echo '{"method":"before_config_generated","params":{"workspace_root":"'"$PWD"'"}}' \
  | devdock-plugin-config-fetcher
```

---

## Regression Checklist

Before merging feat/devdock-oss → main:

- [ ] GCP QE: 11/11 resources healthy
- [ ] AWS: all resources healthy (with credentials)
- [ ] Pipelines: pipeline resources run to completion
- [ ] Test suite: test resources appear correctly
- [ ] --services filter: only specified services active
- [ ] dry-run: config generated, no Tilt started
- [ ] stop: all containers removed, generated files cleaned
- [ ] status: shows running instances
- [ ] infra sql setup: runs standalone
- [ ] cargo test --workspace: 141 tests pass
- [ ] cargo clippy --workspace: 0 warnings
