#!/bin/bash
# DevDock external plugin (bash)
# Protocol: JSON-lines over stdin/stdout

while IFS= read -r line; do
    method=$(echo "$line" | python3 -c "import json,sys; print(json.load(sys.stdin).get('method',''))")

    case "$method" in
        initialize)
            echo '{"status":"ok"}'
            ;;
        emit)
            # YOUR CODE HERE
            echo '{"status":"ok"}'
            ;;
        shutdown)
            echo '{"status":"ok"}'
            exit 0
            ;;
        *)
            echo "{\"status\":\"error\",\"message\":\"unknown method: $method\"}"
            ;;
    esac
done
