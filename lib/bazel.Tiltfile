load("./utils.Tiltfile", "get_bazel_cmd", "get_bazel_flags")

def bazel_build(name, target, watch_dirs = None, type = None, target_is_directory = "True", hot_reload = "enable", copy_runfiles = False):
    """Build a bazel target and watch for changes"""

    is_test_suite = (type == "test_suite")

    # Default to watching the entire workspace if no dirs specified
    if not watch_dirs:
        watch_dirs = ["."]

    local_resource(
        '{}-watch-file-updater'.format(name),
        cmd = '''
        python {}/devdock/core/config/cquery.py {} {} {}
        '''.format(os.getenv('DEVDOCK_ROOT'), os.getenv('DEVDOCK_ROOT'), name, target),
        auto_init = False,
        trigger_mode=TRIGGER_MODE_MANUAL,
        labels = ['watch-files-updater'],
    )

    devdock_root = os.getenv('DEVDOCK_ROOT')

    # Try to generate watch file via cquery (optional — may not exist in OSS mode)
    cquery_script = '{devdock_root}/devdock/core/config/cquery.py'.format(devdock_root=devdock_root)
    watch_file = '{devdock_root}/devdock/runtime_generated/watch_file_{name}.yaml'.format(devdock_root=devdock_root, name=name)
    if os.path.exists(cquery_script):
        local('[ ! -s {watch} ] && python {script} {root} {name} {target} || true'.format(
            watch=watch_file, script=cquery_script, root=devdock_root, name=name, target=target))

    # Read watch file if it exists, otherwise use empty paths
    paths = {}
    if os.path.exists(watch_file):
        paths = read_yaml(watch_file)
    if not paths:
        paths = {}

    # Read profile to get architecture
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    architecture = profile.get('architecture', 'unknown')

    # Run bazel build using local()
    bazel_command = ["bazel"]

    # Add bazelrc flag for Linux systems
    bazel_command.extend(get_bazel_flags())

    bazel_command.append("build")

    if type == "go":
        bazel_command.append("--compilation_mode=dbg")
        # Only add platform flag for cross-compilation (when NOT on Linux)
        if architecture != "linux":
            bazel_command.append("--platforms=@io_bazel_rules_go//go/toolchain:linux_arm64")
    elif type == "java" and architecture != "linux":
        bazel_command.append("--jobs=16")
    elif type == "test_suite":
        type = "python"
        # Only add platform flag for cross-compilation (when NOT on Linux)
        if architecture != "linux":
            bazel_command.append("--platforms=@io_bazel_rules_go//go/toolchain:linux_amd64")


    # Get bazel-bin path
    bazel_info_cmd = ["bazel"]
    bazel_info_cmd.extend(get_bazel_flags())
    bazel_info_cmd.extend(["info", "bazel-bin"])

    if type == "go":
        bazel_info_cmd.append("--compilation_mode=dbg")

    bazel_bin = str(local(' '.join(bazel_info_cmd), quiet=True)).strip()

    if target_is_directory == "False":
        target_path = target.replace(":", "/")
    else:
        target_path = target.split(":")[0]

    binary_name = target.split(":")[-1]

    if type == "go":
        clean_target_path = target_path.lstrip('/')
        # Append the extra directory structure
        target_path = "{}/{}_".format(clean_target_path, binary_name)

    # Join paths using string concatenation since os.path is not available
    full_path = bazel_bin.rstrip('/') + '/' + target_path.lstrip('/')


    bazel_command.append(target)
    bazel_command = ' '.join(bazel_command)
    local('mkdir -p ~/.devdock/{}'.format(name))

    resource_deps_list = []
    if profile.get('infra',{}).get('sql',{}).get('type', 'remote') == 'local':
        if os.getenv('DEVDOCK_RUST_BINARY', '').lower() != 'true':
            resource_deps_list.append('sql-util-build')

    # Non-Go services depend on central-bazel-build for parallel compilation
    if type not in ['go', None]:
        resource_deps_list.append('central-bazel-build')

    local_resource(
        '{}-bazel-build'.format(name),
        cmd = '''
            #!/bin/bash
            set -e
            mkdir -p ~/.devdock/{name}
            SECONDS=0
            {bazel_command}
            bazel_status=$?
            echo "bazel build took $SECONDS seconds"
            SECONDS=0
            if [ $bazel_status -eq 0 ]; then
              if [ "{target_is_directory}" = "True" ]; then
                if [ "{is_test_suite}" = "True" ]; then
                  rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path}/{binary_name}* ~/.devdock/{name}/ || true
                else
                  rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path}/ ~/.devdock/{name} || true
                fi
              elif [ "{target_is_directory}" = "False" ] && [ "{copy_runfiles}" = "True" ]; then
                # Copy only the target's artifacts (explicit list for clarity)
                rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path} ~/.devdock/{name}/ || true
                rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path}.repo_mapping ~/.devdock/{name}/ 2>/dev/null || true
                rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path}.runfiles ~/.devdock/{name}/ 2>/dev/null || true
                rsync -rltD -L --exclude="pip_*" --exclude="rules_python" {full_path}.runfiles_manifest ~/.devdock/{name}/ 2>/dev/null || true
              elif [ "{target_is_directory}" = "False" ]; then
                chmod -R 755 ~/.devdock/{name} && FILENAME=$(basename {full_path}) && mkdir -p ~/.devdock/staging/{name} && cp {full_path} ~/.devdock/staging/{name}/$FILENAME && mv ~/.devdock/staging/{name}/$FILENAME ~/.devdock/{name}/$FILENAME
              fi
              [ "{type}" = "go" ] && tilt trigger {name} || true
              docker images --filter "dangling=true" -q | xargs -r docker rmi || true
            fi
            echo "post-process took $SECONDS seconds"
            exit $bazel_status
        '''.format(name=name, bazel_command=bazel_command, target_is_directory=target_is_directory, full_path=full_path, type=type, devdock_root=devdock_root, binary_name=binary_name, is_test_suite=is_test_suite, copy_runfiles=copy_runfiles),
        resource_deps = resource_deps_list,
        deps = (['../../../../{}'.format(path) for path in paths.get(name, [])] + ['../../../devdock.ini']) if hot_reload == "enable" else [],
        allow_parallel = True,
        labels=['bazel-builds']
    )

    return full_path

# -*- mode: Python -*-

BAZEL_SOURCES_CMD_TEMPLATE = """
  %s cquery 'filter("^//", kind("source file", deps(set(%%s))))'
  """.strip()

def bazel_labels_to_files(labels):
    files = {}
    for l in labels:
        if l.startswith("//external/") or l.startswith("//external:") or l.startswith("@pip") :
            continue
        elif l.startswith("//"):
            l = l[2:]

        path = l.replace(":", "/")
        if path.startswith("/"):
            path = path[1:]

        files[path] = None

    return files.keys()

def bazel_sourcefile_deps(target):
    bazel_cmd = get_bazel_cmd()
    cmd = BAZEL_SOURCES_CMD_TEMPLATE % (bazel_cmd, target)
    output = str(local(cmd, quiet=True))
    return bazel_labels_to_files(output.splitlines())
