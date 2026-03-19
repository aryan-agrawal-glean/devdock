#!/bin/bash
set -e

PARAMS_STR=$(echo ${PARAMS_STR} | sed "s/PROJECT_NUMBER/${PROJECT_NUMBER}/g")
PARAMS_STR=$(echo ${PARAMS_STR} | sed "s/PROJECT_REGION/${PROJECT_REGION}/g")
PARAMS_STR=$(echo ${PARAMS_STR} | sed "s/PROJECT_NAME/${PROJECT_NAME}/g")

# Start up pipelines process
java ${DEBUG:+-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5002} \
  -Dhttp.maxConnections=100 \
  -cp ${PIPELINE_CLASS_PATH:-/app/dataflow_deploy.jar} \
  ${MAIN} \
  --localRun \
  --project=${PROJECT_NAME} \
  ${PARAMS_STR}
