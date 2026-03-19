#!/bin/bash
set -e

PARAMS_STR=$(echo ${PARAMS_STR} | sed "s/PROJECT_NUMBER/${PROJECT_NUMBER}/g")
PARAMS_STR=$(echo ${PARAMS_STR} | sed "s/PROJECT_REGION/${PROJECT_REGION}/g")

# Read pipeline execution mode from environment or default to auto
PIPELINE_EXECUTION=${PIPELINE_EXECUTION:-auto}

# Log the detected configuration for debugging
echo "DLP Pipeline Configuration:"
echo "  MAIN: ${MAIN}"
echo "  PIPELINE_EXECUTION: ${PIPELINE_EXECUTION}"
echo "  PROJECT_NAME: ${PROJECT_NAME}"
echo "  DLP_DOCS_DUMP_PATH: ${DLP_DOCS_DUMP_PATH:-NOT_SET}"
echo "  DLP_REPORT_IDS: ${DLP_REPORT_IDS:-NOT_SET}"
echo "  PROJECT_ID: ${PROJECT_ID:-NOT_SET}"

# Check execution mode for pipeline behavior
case "${PIPELINE_EXECUTION}" in
    "auto")
        echo "Auto execution mode: Starting pipeline immediately with environment variables"
        # Start pipeline directly on container startup
        java ${DEBUG:+-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5002} \
          -Dhttp.maxConnections=100 \
          -cp ${PIPELINE_CLASS_PATH:-/app/dataflow_deploy.jar} \
          ${MAIN} \
          --localRun \
          --project=${PROJECT_NAME} \
          ${PARAMS_STR}
        ;;
    "dedicated")
        echo "Dedicated container mode: Pipeline will be triggered by cron_helper via docker exec"
        echo "Container ready for LocalBeamLauncher execution via docker exec"
        sleep infinity
        ;;
    "inline")
        echo "Inline container mode: Pipeline will be executed in cron_helper container"
        echo "Container not used for execution - waiting indefinitely"
        sleep infinity
        ;;
    *)
        echo "Unknown or remote execution mode: ${PIPELINE_EXECUTION}"
        echo "Container waiting indefinitely (not used for local execution)"
        sleep infinity
        ;;
esac
