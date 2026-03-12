# Skill: setup-scio-test

Set up the scio repository for testing with the new DevDock OSS binary.
Creates a clean branch with only the necessary config files.

## Usage
```
/setup-scio-test
```

## Steps

1. Create test branch in scio:
   ```bash
   cd ~/workspace/scio
   git checkout master
   git pull
   git checkout -b test/devdock-oss-testing
   ```

2. Ensure .devdock/devdock.toml exists:
   ```bash
   mkdir -p .devdock
   cat > .devdock/devdock.toml << 'EOF'
   [project]
   name = "scio"
   domain = "local.glean.com"
   services_dir = "devdock/config/services"
   infra_dir = "devdock/config/infra"

   [registry]
   url = "us-central1-docker.pkg.dev/scio-engineering/glean-images"

   [remote]
   provider = "coder"
   template = "devdock-glean"

   [telemetry]
   enabled = false

   [cloud]
   platform = "google"
   project = "salessavvy-test"
   region = "us-central1"
   EOF
   ```

3. Add requirements field to Python services that need it:
   ```bash
   for svc in $(find devdock/config/services -name "devdock_config.json" -exec grep -l '"type": "python"' {} \;); do
       python3 -c "
   import json
   f = '$svc'
   d = json.load(open(f))
   if 'requirements' not in d:
       d['requirements'] = {
           'linux': 'python_scio/requirements/bazel/linux/requirements.txt',
           'darwin': 'python_scio/requirements/bazel/linux_aarch64/requirements.txt',
           'wheels': 'python_scio/requirements/wheels'
       }
       json.dump(d, open(f, 'w'), indent=2)
       print(f'Updated: {f}')
   "
   done
   ```

4. Verify the devdock binary is the OSS version:
   ```bash
   devdock --version
   devdock init --help  # Should show help (only exists in OSS binary)
   ```

5. List available profiles:
   ```bash
   ls devdock/profiles/*.yaml | head -10
   ```
