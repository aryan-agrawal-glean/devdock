load('ext://uibutton', 'choice_input', 'cmd_button', 'location')

def export_logs(resource, service):
    local_resource(
        'log_exporter_{}'.format(service),
        serve_cmd =
        '''
        echo "\n\n"
        echo "***********************************************"
        echo "* Streaming {} logs to ~/.devdock/{}/logs.log *"
        echo "***********************************************"
        LAST_TIMESTAMP=$(tail -20 ~/.devdock/{}/logs.log 2>/dev/null | grep -E '^[0-9]{{{{4}}}}-[0-9]{{{{2}}}}-[0-9]{{{{2}}}}T[0-9]{{{{2}}}}:[0-9]{{{{2}}}}:[0-9]{{{{2}}}}' | tail -1 | cut -d' ' -f1 2>/dev/null || echo "1m")

        while true; do
          CONTAINER_ID=$(docker ps --filter "label=devdock.service={}" --format "table {{{{.ID}}}}" | tail -n +2 | head -1)
          if [ -n "$CONTAINER_ID" ]; then
            echo "$(date): Starting to stream logs for container $CONTAINER_ID..." >> ~/.devdock/{}/logs.log
            docker logs --timestamps --follow --since "$LAST_TIMESTAMP" "$CONTAINER_ID" >> ~/.devdock/{}/logs.log 2>&1
            echo "$(date): Docker logs command exited, retrying in 10s..." >> ~/.devdock/{}/logs.log
            sleep 10
          else
            echo "$(date): No container found for resource {}, retrying in 10s..." >> ~/.devdock/{}/logs.log
            sleep 10
          fi
        done
        '''.format(service, service, service, service, service, service, service, service, service),
        allow_parallel=True,
        auto_init=True,
        labels=['log_exporter'])
