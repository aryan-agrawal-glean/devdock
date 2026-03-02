FROM $base_image

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    curl \
    git \
    wget \
    build-essential \
    coreutils \
    && rm -rf /var/lib/apt/lists/*

# Install Go
RUN wget https://go.dev/dl/go1.25.1.linux-$go_arch.tar.gz \
    && tar -C /usr/local -xzf go1.25.1.linux-$go_arch.tar.gz \
    && rm go1.25.1.linux-$go_arch.tar.gz

# Set Go environment variables
ENV PATH=/usr/bin:/usr/local/bin:/bin:/usr/local/go/bin
ENV GOBIN=/usr/local/go/bin
ENV GOROOT=/usr/local/go
ENV GO111MODULE=on

# Install Delve (use go install for reliability)
RUN go install github.com/go-delve/delve/cmd/dlv@latest && \
    cp $(go env GOPATH)/bin/dlv /usr/local/bin/ || \
    cp /usr/local/go/bin/dlv /usr/local/bin/

# Copy entrypoint script if specified
$entrypoint_copy

# Set environment variables
$env_vars

# Expose ports
EXPOSE $port
$debugport_expose

# Make entrypoint script executable
$entrypoint_chmod

$run_commands

# Set entrypoint
$entrypoint_cmd
