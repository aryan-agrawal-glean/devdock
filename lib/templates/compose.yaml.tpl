services:
  $service:
    build:
      context: .
      dockerfile: Dockerfile
    container_name: devdock-$service
    ports:
      - "$port:$port"
      - "$debug_port:$debug_port"
    environment:
$compose_env
    volumes:
      - ~/.devdock/auth/application_default_credentials.json:/tmp/.config/gcloud/application_default_credentials.json
      - ${DEVDOCK_ROOT}/config/:/config
      - $mount_binary_path
$extra_mounts
    restart: unless-stopped
    networks:
      devdock_network:
        aliases:
          - $service.devdock.local
$healthcheck
    labels:
      devdock.service: "$service"
      devdock.type: "service"
    $debugport_hosts

networks:
  devdock_network:
    external: true
