# DaemonEye Docker Deployment Guide

This guide provides comprehensive instructions for deploying DaemonEye using Docker and Docker Compose, including containerization, orchestration, and production deployment strategies.

---

## Table of Contents

[TOC]

---

## Docker Overview

DaemonEye is designed to run efficiently in containerized environments, providing:

- **Isolation**: Process monitoring within container boundaries
- **Scalability**: Easy horizontal scaling and load balancing
- **Portability**: Consistent deployment across different environments
- **Security**: Container-based privilege isolation
- **Orchestration**: Integration with Kubernetes and other orchestration platforms

### Container Architecture

DaemonEye uses a multi-container architecture:

- **procmond**: Privileged process monitoring daemon
- **daemoneye-agent**: User-space orchestrator and alerting
- **daemoneye-cli**: Command-line interface and management
- **Security Center**: Web-based management interface (Business/Enterprise tiers)

## Container Images

### Official Images

**Core Images**:

```bash
# Process monitoring daemon
docker pull daemoneye/procmond:latest
docker pull daemoneye/procmond:1.0.0

# Agent orchestrator
docker pull daemoneye/daemoneye-agent:latest
docker pull daemoneye/daemoneye-agent:1.0.0

# CLI interface
docker pull daemoneye/daemoneye-cli:latest
docker pull daemoneye/daemoneye-cli:1.0.0

# Security Center (Business/Enterprise)
docker pull daemoneye/security-center:latest
docker pull daemoneye/security-center:1.0.0
```

**Image Tags**:

- `latest`: Latest stable release
- `1.0.0`: Specific version
- `1.0.0-alpine`: Alpine Linux variant (smaller size)
- `1.0.0-debian`: Debian-based variant
- `dev`: Development builds
- `rc-1.0.0`: Release candidate

### Image Variants

**Alpine Linux** (Recommended):

```dockerfile
# Smaller size, security-focused
FROM alpine:3.18
RUN apk add --no-cache ca-certificates
COPY procmond /usr/local/bin/
ENTRYPOINT ["procmond"]
```

**Debian**:

```dockerfile
# Full-featured, larger size
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY procmond /usr/local/bin/
ENTRYPOINT ["procmond"]
```

**Distroless**:

```dockerfile
# Minimal attack surface
FROM gcr.io/distroless/cc-debian12
COPY procmond /usr/local/bin/
ENTRYPOINT ["procmond"]
```

## Basic Docker Deployment

### Single Container Deployment

**Run Process Monitor**:

```bash
# Basic run
docker run -d \
  --name daemoneye-procmond \
  --privileged \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  daemoneye/procmond:latest

# With custom configuration
docker run -d \
  --name daemoneye-procmond \
  --privileged \
  -v /etc/daemoneye:/config \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  -e DaemonEye_LOG_LEVEL=info \
  daemoneye/procmond:latest --config /config/config.yaml
```

**Run Agent**:

```bash
# Basic run
docker run -d \
  --name daemoneye-agent \
  --link daemoneye-procmond:procmond \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  daemoneye/daemoneye-agent:latest

# With custom configuration
docker run -d \
  --name daemoneye-agent \
  --link daemoneye-procmond:procmond \
  -v /etc/daemoneye:/config \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  -e DaemonEye_LOG_LEVEL=info \
  daemoneye/daemoneye-agent:latest --config /config/config.yaml
```

**Run CLI**:

```bash
# Interactive CLI
docker run -it \
  --rm \
  --link daemoneye-agent:agent \
  -v /var/lib/daemoneye:/data \
  daemoneye/daemoneye-cli:latest

# Execute specific command
docker run --rm \
  --link daemoneye-agent:agent \
  -v /var/lib/daemoneye:/data \
  daemoneye/daemoneye-cli:latest query "SELECT * FROM processes LIMIT 10"
```

### Multi-Container Deployment

**Create Network**:

```bash
# Create custom network
docker network create daemoneye-network

# Run with custom network
docker run -d \
  --name daemoneye-procmond \
  --network daemoneye-network \
  --privileged \
  -v /var/lib/daemoneye:/data \
  daemoneye/procmond:latest

docker run -d \
  --name daemoneye-agent \
  --network daemoneye-network \
  -v /var/lib/daemoneye:/data \
  daemoneye/daemoneye-agent:latest
```

## Docker Compose Deployment

### Basic Docker Compose

**docker-compose.yml**:

```yaml
version: '3.8'

services:
  procmond:
    image: daemoneye/procmond:latest
    container_name: daemoneye-procmond
    privileged: true
    volumes:
      - /var/lib/daemoneye:/data
      - /var/log/daemoneye:/logs
      - ./config:/config:ro
    environment:
      - DaemonEye_LOG_LEVEL=info
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network

  daemoneye-agent:
    image: daemoneye/daemoneye-agent:latest
    container_name: daemoneye-agent
    depends_on:
      - procmond
    volumes:
      - /var/lib/daemoneye:/data
      - /var/log/daemoneye:/logs
      - ./config:/config:ro
    environment:
      - DaemonEye_LOG_LEVEL=info
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network

  daemoneye-cli:
    image: daemoneye/daemoneye-cli:latest
    container_name: daemoneye-cli
    depends_on:
      - daemoneye-agent
    volumes:
      - /var/lib/daemoneye:/data
      - ./config:/config:ro
    environment:
      - DaemonEye_DATA_DIR=/data
    command: [--help]
    restart: no
    networks:
      - daemoneye-network

networks:
  daemoneye-network:
    driver: bridge

volumes:
  daemoneye-data:
    driver: local
  daemoneye-logs:
    driver: local
```

### Production Docker Compose

**docker-compose.prod.yml**:

```yaml
version: '3.8'

services:
  procmond:
    image: daemoneye/procmond:1.0.0
    container_name: daemoneye-procmond
    privileged: true
    user: 1000:1000
    volumes:
      - daemoneye-data:/data
      - daemoneye-logs:/logs
      - ./config/procmond.yaml:/config/config.yaml:ro
      - ./rules:/rules:ro
    environment:
      - DaemonEye_LOG_LEVEL=info
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
      - DaemonEye_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    healthcheck:
      test: [CMD, procmond, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  daemoneye-agent:
    image: daemoneye/daemoneye-agent:1.0.0
    container_name: daemoneye-agent
    depends_on:
      procmond:
        condition: service_healthy
    user: 1000:1000
    volumes:
      - daemoneye-data:/data
      - daemoneye-logs:/logs
      - ./config/daemoneye-agent.yaml:/config/config.yaml:ro
    environment:
      - DaemonEye_LOG_LEVEL=info
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
      - DaemonEye_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    healthcheck:
      test: [CMD, daemoneye-agent, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  security-center:
    image: daemoneye/security-center:1.0.0
    container_name: daemoneye-security-center
    depends_on:
      daemoneye-agent:
        condition: service_healthy
    user: 1000:1000
    volumes:
      - daemoneye-data:/data
      - ./config/security-center.yaml:/config/config.yaml:ro
    environment:
      - DaemonEye_LOG_LEVEL=info
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_AGENT_ENDPOINT=tcp://daemoneye-agent:8080
      - DaemonEye_WEB_PORT=8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    ports:
      - 8080:8080
    healthcheck:
      test: [CMD, curl, -f, http://localhost:8080/health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  nginx:
    image: nginx:alpine
    container_name: daemoneye-nginx
    depends_on:
      security-center:
        condition: service_healthy
    volumes:
      - ./nginx/nginx.conf:/etc/nginx/nginx.conf:ro
      - ./nginx/ssl:/etc/nginx/ssl:ro
    ports:
      - 80:80
      - 443:443
    restart: unless-stopped
    networks:
      - daemoneye-network

networks:
  daemoneye-network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16

volumes:
  daemoneye-data:
    driver: local
  daemoneye-logs:
    driver: local
```

### Development Docker Compose

**docker-compose.dev.yml**:

```yaml
version: '3.8'

services:
  procmond:
    build:
      context: .
      dockerfile: procmond/Dockerfile
    container_name: daemoneye-procmond-dev
    privileged: true
    volumes:
      - ./data:/data
      - ./logs:/logs
      - ./config:/config:ro
      - ./rules:/rules:ro
    environment:
      - DaemonEye_LOG_LEVEL=debug
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
      - DaemonEye_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network

  daemoneye-agent:
    build:
      context: .
      dockerfile: daemoneye-agent/Dockerfile
    container_name: daemoneye-agent-dev
    depends_on:
      - procmond
    volumes:
      - ./data:/data
      - ./logs:/logs
      - ./config:/config:ro
    environment:
      - DaemonEye_LOG_LEVEL=debug
      - DaemonEye_DATA_DIR=/data
      - DaemonEye_LOG_DIR=/logs
      - DaemonEye_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network

  daemoneye-cli:
    build:
      context: .
      dockerfile: daemoneye-cli/Dockerfile
    container_name: daemoneye-cli-dev
    depends_on:
      - daemoneye-agent
    volumes:
      - ./data:/data
      - ./config:/config:ro
    environment:
      - DaemonEye_DATA_DIR=/data
    command: [--help]
    restart: no
    networks:
      - daemoneye-network

networks:
  daemoneye-network:
    driver: bridge
```

## Production Deployment

### Production Configuration

**Environment Variables**:

```bash
# .env file
DaemonEye_VERSION=1.0.0
DaemonEye_LOG_LEVEL=info
DaemonEye_DATA_DIR=/var/lib/daemoneye
DaemonEye_LOG_DIR=/var/log/daemoneye
DaemonEye_CONFIG_DIR=/etc/daemoneye

# Database settings
DATABASE_PATH=/var/lib/daemoneye/processes.db
DATABASE_RETENTION_DAYS=30

# Alerting settings
ALERTING_SYSLOG_ENABLED=true
ALERTING_SYSLOG_FACILITY=daemon
ALERTING_WEBHOOK_ENABLED=false
ALERTING_WEBHOOK_URL=https://alerts.example.com/webhook
ALERTING_WEBHOOK_TOKEN=${WEBHOOK_TOKEN}

# Security settings
SECURITY_ENABLE_PRIVILEGE_DROPPING=true
SECURITY_DROP_TO_USER=1000
SECURITY_DROP_TO_GROUP=1000
SECURITY_ENABLE_AUDIT_LOGGING=true

# Performance settings
APP_SCAN_INTERVAL_MS=60000
APP_BATCH_SIZE=1000
APP_MAX_MEMORY_MB=512
APP_MAX_CPU_PERCENT=5.0
```

**Production Docker Compose**:

```yaml
version: '3.8'

services:
  procmond:
    image: daemoneye/procmond:${DaemonEye_VERSION}
    container_name: daemoneye-procmond
    privileged: true
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - daemoneye-data:/data
      - daemoneye-logs:/logs
      - ./config/procmond.yaml:/config/config.yaml:ro
      - ./rules:/rules:ro
    environment:
      - DaemonEye_LOG_LEVEL=${DaemonEye_LOG_LEVEL}
      - DaemonEye_DATA_DIR=${DaemonEye_DATA_DIR}
      - DaemonEye_LOG_DIR=${DaemonEye_LOG_DIR}
      - DaemonEye_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    healthcheck:
      test: [CMD, procmond, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
    logging:
      driver: json-file
      options:
        max-size: 100m
        max-file: '5'

  daemoneye-agent:
    image: daemoneye/daemoneye-agent:${DaemonEye_VERSION}
    container_name: daemoneye-agent
    depends_on:
      procmond:
        condition: service_healthy
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - daemoneye-data:/data
      - daemoneye-logs:/logs
      - ./config/daemoneye-agent.yaml:/config/config.yaml:ro
    environment:
      - DaemonEye_LOG_LEVEL=${DaemonEye_LOG_LEVEL}
      - DaemonEye_DATA_DIR=${DaemonEye_DATA_DIR}
      - DaemonEye_LOG_DIR=${DaemonEye_LOG_DIR}
      - DaemonEye_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    healthcheck:
      test: [CMD, daemoneye-agent, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
    logging:
      driver: json-file
      options:
        max-size: 100m
        max-file: '5'

  security-center:
    image: daemoneye/security-center:${DaemonEye_VERSION}
    container_name: daemoneye-security-center
    depends_on:
      daemoneye-agent:
        condition: service_healthy
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - daemoneye-data:/data
      - ./config/security-center.yaml:/config/config.yaml:ro
    environment:
      - DaemonEye_LOG_LEVEL=${DaemonEye_LOG_LEVEL}
      - DaemonEye_DATA_DIR=${DaemonEye_DATA_DIR}
      - DaemonEye_AGENT_ENDPOINT=tcp://daemoneye-agent:8080
      - DaemonEye_WEB_PORT=8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - daemoneye-network
    ports:
      - 8080:8080
    healthcheck:
      test: [CMD, curl, -f, http://localhost:8080/health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
    logging:
      driver: json-file
      options:
        max-size: 100m
        max-file: '5'

networks:
  daemoneye-network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16

volumes:
  daemoneye-data:
    driver: local
  daemoneye-logs:
    driver: local
```

### Deployment Scripts

**deploy.sh**:

```bash
#!/bin/bash

set -e

# Configuration
COMPOSE_FILE="docker-compose.prod.yml"
ENV_FILE=".env"
BACKUP_DIR="/backup/daemoneye"
LOG_DIR="/var/log/daemoneye"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."

    if ! command -v docker &> /dev/null; then
        log_error "Docker is not installed"
        exit 1
    fi

    if ! command -v docker-compose &> /dev/null; then
        log_error "Docker Compose is not installed"
        exit 1
    fi

    if [ ! -f "$ENV_FILE" ]; then
        log_error "Environment file $ENV_FILE not found"
        exit 1
    fi

    if [ ! -f "$COMPOSE_FILE" ]; then
        log_error "Compose file $COMPOSE_FILE not found"
        exit 1
    fi

    log_info "Prerequisites check passed"
}

# Backup existing deployment
backup_deployment() {
    log_info "Backing up existing deployment..."

    if [ -d "$BACKUP_DIR" ]; then
        rm -rf "$BACKUP_DIR"
    fi

    mkdir -p "$BACKUP_DIR"

    # Backup data volumes
    if docker volume ls | grep -q daemoneye-data; then
        docker run --rm -v daemoneye-data:/data -v "$BACKUP_DIR":/backup alpine tar czf /backup/data.tar.gz -C /data .
    fi

    # Backup logs
    if [ -d "$LOG_DIR" ]; then
        cp -r "$LOG_DIR" "$BACKUP_DIR/logs"
    fi

    log_info "Backup completed"
}

# Deploy DaemonEye
deploy_daemoneye() {
    log_info "Deploying DaemonEye..."

    # Pull latest images
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" pull

    # Stop existing services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" down

    # Start services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" up -d

    # Wait for services to be healthy
    log_info "Waiting for services to be healthy..."
    sleep 30

    # Check service health
    if docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" ps | grep -q "unhealthy"; then
        log_error "Some services are unhealthy"
        docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" logs
        exit 1
    fi

    log_info "Deployment completed successfully"
}

# Main execution
main() {
    log_info "Starting DaemonEye deployment..."

    check_prerequisites
    backup_deployment
    deploy_daemoneye

    log_info "DaemonEye deployment completed successfully"
}

# Run main function
main "$@"
```

**rollback.sh**:

```bash
#!/bin/bash

set -e

# Configuration
COMPOSE_FILE="docker-compose.prod.yml"
ENV_FILE=".env"
BACKUP_DIR="/backup/daemoneye"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Rollback deployment
rollback_deployment() {
    log_info "Rolling back DaemonEye deployment..."

    # Stop current services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" down

    # Restore data volumes
    if [ -f "$BACKUP_DIR/data.tar.gz" ]; then
        docker run --rm -v daemoneye-data:/data -v "$BACKUP_DIR":/backup alpine tar xzf /backup/data.tar.gz -C /data
    fi

    # Restore logs
    if [ -d "$BACKUP_DIR/logs" ]; then
        cp -r "$BACKUP_DIR/logs"/* /var/log/daemoneye/
    fi

    # Start services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" up -d

    log_info "Rollback completed"
}

# Main execution
main() {
    log_info "Starting DaemonEye rollback..."

    rollback_deployment

    log_info "DaemonEye rollback completed successfully"
}

# Run main function
main "$@"
```

## Kubernetes Deployment

### Kubernetes Manifests

**Namespace**:

```yaml
# namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: daemoneye
  labels:
    name: daemoneye
```

**ConfigMap**:

```yaml
# configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: daemoneye-config
  namespace: daemoneye
data:
  procmond.yaml: |
    app:
      scan_interval_ms: 30000
      batch_size: 1000
      log_level: info
      data_dir: /data
      log_dir: /logs
    database:
      path: /data/processes.db
      retention_days: 30
    security:
      enable_privilege_dropping: true
      drop_to_user: 1000
      drop_to_group: 1000

  daemoneye-agent.yaml: |
    app:
      scan_interval_ms: 30000
      batch_size: 1000
      log_level: info
      data_dir: /data
      log_dir: /logs
    database:
      path: /data/processes.db
      retention_days: 30
    alerting:
      enabled: true
      sinks:
        - type: syslog
          enabled: true
          facility: daemon
        - type: webhook
          enabled: true
          url: http://daemoneye-webhook:8080/webhook
```

**Secret**:

```yaml
# secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: daemoneye-secrets
  namespace: daemoneye
type: Opaque
data:
  webhook-token: <base64-encoded-token>
  database-encryption-key: <base64-encoded-key>
```

**PersistentVolumeClaim**:

```yaml
# pvc.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: daemoneye-data
  namespace: daemoneye
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 10Gi
  storageClassName: fast-ssd
```

**DaemonSet for procmond**:

```yaml
# procmond-daemonset.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: daemoneye-procmond
  namespace: daemoneye
spec:
  selector:
    matchLabels:
      app: daemoneye-procmond
  template:
    metadata:
      labels:
        app: daemoneye-procmond
    spec:
      serviceAccountName: daemoneye-procmond
      containers:
        - name: procmond
          image: daemoneye/procmond:1.0.0
          imagePullPolicy: IfNotPresent
          securityContext:
            privileged: true
            runAsUser: 1000
            runAsGroup: 1000
          volumeMounts:
            - name: config
              mountPath: /config
              readOnly: true
            - name: data
              mountPath: /data
            - name: logs
              mountPath: /logs
            - name: rules
              mountPath: /rules
              readOnly: true
          env:
            - name: DaemonEye_LOG_LEVEL
              value: info
            - name: DaemonEye_DATA_DIR
              value: /data
            - name: DaemonEye_LOG_DIR
              value: /logs
          command: [procmond]
          args: [--config, /config/procmond.yaml]
          resources:
            requests:
              memory: 256Mi
              cpu: 100m
            limits:
              memory: 512Mi
              cpu: 500m
          livenessProbe:
            exec:
              command:
                - procmond
                - health
            initialDelaySeconds: 30
            periodSeconds: 30
            timeoutSeconds: 10
            failureThreshold: 3
          readinessProbe:
            exec:
              command:
                - procmond
                - health
            initialDelaySeconds: 10
            periodSeconds: 10
            timeoutSeconds: 5
            failureThreshold: 3
      volumes:
        - name: config
          configMap:
            name: daemoneye-config
        - name: data
          persistentVolumeClaim:
            claimName: daemoneye-data
        - name: logs
          emptyDir: {}
        - name: rules
          configMap:
            name: daemoneye-rules
      tolerations:
        - key: node-role.kubernetes.io/master
          operator: Exists
          effect: NoSchedule
        - key: node-role.kubernetes.io/control-plane
          operator: Exists
          effect: NoSchedule
```

**Deployment for daemoneye-agent**:

```yaml
# daemoneye-agent-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: daemoneye-agent
  namespace: daemoneye
spec:
  replicas: 1
  selector:
    matchLabels:
      app: daemoneye-agent
  template:
    metadata:
      labels:
        app: daemoneye-agent
    spec:
      serviceAccountName: daemoneye-agent
      containers:
        - name: daemoneye-agent
          image: daemoneye/daemoneye-agent:1.0.0
          imagePullPolicy: IfNotPresent
          securityContext:
            runAsUser: 1000
            runAsGroup: 1000
          volumeMounts:
            - name: config
              mountPath: /config
              readOnly: true
            - name: data
              mountPath: /data
            - name: logs
              mountPath: /logs
          env:
            - name: DaemonEye_LOG_LEVEL
              value: info
            - name: DaemonEye_DATA_DIR
              value: /data
            - name: DaemonEye_LOG_DIR
              value: /logs
            - name: DaemonEye_PROCMOND_ENDPOINT
              value: tcp://daemoneye-procmond:8080
          command: [daemoneye-agent]
          args: [--config, /config/daemoneye-agent.yaml]
          resources:
            requests:
              memory: 512Mi
              cpu: 200m
            limits:
              memory: 1Gi
              cpu: 1000m
          livenessProbe:
            exec:
              command:
                - daemoneye-agent
                - health
            initialDelaySeconds: 30
            periodSeconds: 30
            timeoutSeconds: 10
            failureThreshold: 3
          readinessProbe:
            exec:
              command:
                - daemoneye-agent
                - health
            initialDelaySeconds: 10
            periodSeconds: 10
            timeoutSeconds: 5
            failureThreshold: 3
      volumes:
        - name: config
          configMap:
            name: daemoneye-config
        - name: data
          persistentVolumeClaim:
            claimName: daemoneye-data
        - name: logs
          emptyDir: {}
```

**Service**:

```yaml
# service.yaml
apiVersion: v1
kind: Service
metadata:
  name: daemoneye-agent
  namespace: daemoneye
spec:
  selector:
    app: daemoneye-agent
  ports:
    - name: http
      port: 8080
      targetPort: 8080
      protocol: TCP
  type: ClusterIP
```

**ServiceAccount and RBAC**:

```yaml
# rbac.yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: daemoneye-procmond
  namespace: daemoneye
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: daemoneye-agent
  namespace: daemoneye
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: daemoneye-procmond
rules:
- apiGroups: [""]
  resources: ["nodes", "pods"]
  verbs: ["get", "list", "watch"]
- apiGroups: [""]
  resources: ["pods/exec"]
  verbs: ["create"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: daemoneye-procmond
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: daemoneye-procmond
subjects:
- kind: ServiceAccount
  name: daemoneye-procmond
  namespace: daemoneye
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: daemoneye-agent
rules:
- apiGroups: [""]
  resources: ["pods", "services", "endpoints"]
  verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: daemoneye-agent
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: daemoneye-agent
subjects:
- kind: ServiceAccount
  name: daemoneye-agent
  namespace: daemoneye
```

### Helm Chart

**Chart.yaml**:

```yaml
# Chart.yaml
apiVersion: v2
name: daemoneye
description: DaemonEye Security Monitoring Agent
type: application
version: 1.0.0
appVersion: 1.0.0
keywords:
  - security
  - monitoring
  - processes
  - threat-detection
home: https://daemoneye.com
sources:
  - https://github.com/daemoneye/daemoneye
maintainers:
  - name: DaemonEye Team
    email: team@daemoneye.com
```

**values.yaml**:

```yaml
# values.yaml
image:
  repository: daemoneye
  tag: 1.0.0
  pullPolicy: IfNotPresent

replicaCount: 1

serviceAccount:
  create: true
  annotations: {}
  name: ''

podSecurityContext:
  runAsUser: 1000
  runAsGroup: 1000
  fsGroup: 1000

securityContext:
  allowPrivilegeEscalation: false
  capabilities:
    drop:
      - ALL
  readOnlyRootFilesystem: true
  runAsNonRoot: true
  runAsUser: 1000

service:
  type: ClusterIP
  port: 8080

ingress:
  enabled: false
  className: ''
  annotations: {}
  hosts:
    - host: daemoneye.example.com
      paths:
        - path: /
          pathType: Prefix
  tls: []

resources:
  limits:
    cpu: 1000m
    memory: 1Gi
  requests:
    cpu: 200m
    memory: 512Mi

autoscaling:
  enabled: false
  minReplicas: 1
  maxReplicas: 10
  targetCPUUtilizationPercentage: 80
  targetMemoryUtilizationPercentage: 80

nodeSelector: {}

tolerations: []

affinity: {}

persistence:
  enabled: true
  storageClass: ''
  accessMode: ReadWriteOnce
  size: 10Gi

config:
  app:
    scan_interval_ms: 30000
    batch_size: 1000
    log_level: info
  database:
    retention_days: 30
  alerting:
    enabled: true
    sinks:
      - type: syslog
        enabled: true
        facility: daemon

secrets: {}

monitoring:
  enabled: false
  serviceMonitor:
    enabled: false
    namespace: ''
    interval: 30s
    scrapeTimeout: 10s
```

## Security Considerations

### Container Security

**Security Context**:

```yaml
securityContext:
  runAsUser: 1000
  runAsGroup: 1000
  runAsNonRoot: true
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop:
      - ALL
    add:
      - CAP_SYS_PTRACE # Only for procmond
```

**Privileged Containers**:

```yaml
# Only procmond needs privileged access
securityContext:
  privileged: true
  capabilities:
    add:
      - CAP_SYS_PTRACE
      - CAP_SYS_ADMIN
```

**Network Security**:

```yaml
# Network policies
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: daemoneye-network-policy
  namespace: daemoneye
spec:
  podSelector:
    matchLabels:
      app: daemoneye
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              name: daemoneye
      ports:
        - protocol: TCP
          port: 8080
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              name: daemoneye
      ports:
        - protocol: TCP
          port: 8080
```

### Image Security

**Image Scanning**:

```bash
# Scan images for vulnerabilities
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  aquasec/trivy image daemoneye/procmond:1.0.0

# Scan with specific severity levels
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  aquasec/trivy image --severity HIGH,CRITICAL daemoneye/procmond:1.0.0
```

**Image Signing**:

```bash
# Sign images with Docker Content Trust
export DOCKER_CONTENT_TRUST=1
docker push daemoneye/procmond:1.0.0
```

**Multi-stage Builds**:

```dockerfile
# Multi-stage build for smaller attack surface
FROM rust:1.91-alpine AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine:3.18
RUN apk add --no-cache ca-certificates
COPY --from=builder /app/target/release/procmond /usr/local/bin/
USER 1000:1000
ENTRYPOINT ["procmond"]
```

## Monitoring and Logging

### Container Monitoring

**Prometheus Metrics**:

```yaml
# Enable metrics collection
observability:
  enable_metrics: true
  metrics_port: 9090
  metrics_path: /metrics
```

**Grafana Dashboard**:

```json
{
  "dashboard": {
    "title": "DaemonEye Monitoring",
    "panels": [
      {
        "title": "Process Collection Rate",
        "type": "graph",
        "targets": [
          {
            "expr": "rate(daemoneye_processes_collected_total[5m])",
            "legendFormat": "Processes/sec"
          }
        ]
      },
      {
        "title": "Memory Usage",
        "type": "graph",
        "targets": [
          {
            "expr": "daemoneye_memory_usage_bytes",
            "legendFormat": "Memory Usage"
          }
        ]
      }
    ]
  }
}
```

### Log Aggregation

**Fluentd Configuration**:

```yaml
# fluentd-configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: fluentd-config
  namespace: daemoneye
data:
  fluent.conf: |
    <source>
      @type tail
      path /var/log/daemoneye/*.log
      pos_file /var/log/fluentd/daemoneye.log.pos
      tag daemoneye.*
      format json
    </source>

    <match daemoneye.**>
      @type elasticsearch
      host elasticsearch.logging.svc.cluster.local
      port 9200
      index_name daemoneye
      type_name _doc
    </match>
```

**ELK Stack Integration**:

```yaml
# elasticsearch-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: elasticsearch
  namespace: logging
spec:
  replicas: 1
  selector:
    matchLabels:
      app: elasticsearch
  template:
    metadata:
      labels:
        app: elasticsearch
    spec:
      containers:
        - name: elasticsearch
          image: docker.elastic.co/elasticsearch/elasticsearch:8.8.0
          env:
            - name: discovery.type
              value: single-node
            - name: xpack.security.enabled
              value: 'false'
          ports:
            - containerPort: 9200
          resources:
            requests:
              memory: 1Gi
              cpu: 500m
            limits:
              memory: 2Gi
              cpu: 1000m
```

## Troubleshooting

### Common Issues

**Container Won't Start**:

```bash
# Check container logs
docker logs daemoneye-procmond

# Check container status
docker ps -a

# Check resource usage
docker stats daemoneye-procmond
```

**Permission Denied**:

```bash
# Check file permissions
docker exec daemoneye-procmond ls -la /data

# Fix permissions
docker exec daemoneye-procmond chown -R 1000:1000 /data
```

**Network Issues**:

```bash
# Check network connectivity
docker exec daemoneye-agent ping daemoneye-procmond

# Check DNS resolution
docker exec daemoneye-agent nslookup daemoneye-procmond
```

**Database Issues**:

```bash
# Check database status
docker exec daemoneye-agent daemoneye-cli database status

# Check database integrity
docker exec daemoneye-agent daemoneye-cli database integrity-check

# Repair database
docker exec daemoneye-agent daemoneye-cli database repair
```

### Debug Mode

**Enable Debug Logging**:

```yaml
# docker-compose.yml
services:
  procmond:
    environment:
      - DaemonEye_LOG_LEVEL=debug
    command: [procmond, --config, /config/config.yaml, --log-level, debug]
```

**Debug Container**:

```bash
# Run debug container
docker run -it --rm --privileged \
  -v /var/lib/daemoneye:/data \
  -v /var/log/daemoneye:/logs \
  daemoneye/procmond:latest /bin/sh

# Check system capabilities
docker run --rm --privileged daemoneye/procmond:latest capsh --print
```

### Performance Issues

**High CPU Usage**:

```bash
# Check CPU usage
docker stats daemoneye-procmond

# Reduce scan frequency
docker exec daemoneye-procmond daemoneye-cli config set app.scan_interval_ms 120000
```

**High Memory Usage**:

```bash
# Check memory usage
docker stats daemoneye-procmond

# Limit memory usage
docker run -d --memory=512m daemoneye/procmond:latest
```

**Slow Database Operations**:

```bash
# Check database performance
docker exec daemoneye-agent daemoneye-cli database query-stats

# Optimize database
docker exec daemoneye-agent daemoneye-cli database optimize
```

### Health Checks

**Container Health**:

```bash
# Check container health
docker inspect daemoneye-procmond | jq '.[0].State.Health'

# Run health check manually
docker exec daemoneye-procmond procmond health
```

**Service Health**:

```bash
# Check service health
curl http://localhost:8080/health

# Check metrics
curl http://localhost:9090/metrics
```

---

*This Docker deployment guide provides comprehensive instructions for containerizing and deploying DaemonEye. For additional help, consult the troubleshooting section or contact support.*
