services:
  $service:
    build:
      context: .
      dockerfile: Dockerfile
      pull: true
    container_name: devdock-$service
    environment:
$compose_env
    volumes:
      - ~/.devdock/auth/application_default_credentials.json:/tmp/.config/gcloud/application_default_credentials.json
      - ${DEVDOCK_ROOT}/config/:/config
      - $mount_binary_path
      - ~/.config/gcloud/:/root/.config/gcloud/
$extra_mounts
    restart: no
    networks:
      devdock_network:
        aliases:
          - $service.devdock.local
    extra_hosts:
        - host.docker.internal:host-gateway
$debugport_hosts
$healthcheck
    labels:
      devdock.service: "$service"
      devdock.type: "service"

networks:
  devdock_network:
    external: true
