# SentinelD Docker Deployment Guide

This guide provides comprehensive instructions for deploying SentinelD using Docker and Docker Compose, including containerization, orchestration, and production deployment strategies.

---

## Table of Contents

[TOC]

---

## Docker Overview

SentinelD is designed to run efficiently in containerized environments, providing:

- **Isolation**: Process monitoring within container boundaries
- **Scalability**: Easy horizontal scaling and load balancing
- **Portability**: Consistent deployment across different environments
- **Security**: Container-based privilege isolation
- **Orchestration**: Integration with Kubernetes and other orchestration platforms

### Container Architecture

SentinelD uses a multi-container architecture:

- **procmond**: Privileged process monitoring daemon
- **sentinelagent**: User-space orchestrator and alerting
- **sentinelcli**: Command-line interface and management
- **Security Center**: Web-based management interface (Business/Enterprise tiers)

## Container Images

### Official Images

**Core Images**:

```bash
# Process monitoring daemon
docker pull sentineld/procmond:latest
docker pull sentineld/procmond:1.0.0

# Agent orchestrator
docker pull sentineld/sentinelagent:latest
docker pull sentineld/sentinelagent:1.0.0

# CLI interface
docker pull sentineld/sentinelcli:latest
docker pull sentineld/sentinelcli:1.0.0

# Security Center (Business/Enterprise)
docker pull sentineld/security-center:latest
docker pull sentineld/security-center:1.0.0
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
  --name sentineld-procmond \
  --privileged \
  -v /var/lib/sentineld:/data \
  -v /var/log/sentineld:/logs \
  sentineld/procmond:latest

# With custom configuration
docker run -d \
  --name sentineld-procmond \
  --privileged \
  -v /etc/sentineld:/config \
  -v /var/lib/sentineld:/data \
  -v /var/log/sentineld:/logs \
  -e SENTINELD_LOG_LEVEL=info \
  sentineld/procmond:latest --config /config/config.yaml
```

**Run Agent**:

```bash
# Basic run
docker run -d \
  --name sentineld-agent \
  --link sentineld-procmond:procmond \
  -v /var/lib/sentineld:/data \
  -v /var/log/sentineld:/logs \
  sentineld/sentinelagent:latest

# With custom configuration
docker run -d \
  --name sentineld-agent \
  --link sentineld-procmond:procmond \
  -v /etc/sentineld:/config \
  -v /var/lib/sentineld:/data \
  -v /var/log/sentineld:/logs \
  -e SENTINELD_LOG_LEVEL=info \
  sentineld/sentinelagent:latest --config /config/config.yaml
```

**Run CLI**:

```bash
# Interactive CLI
docker run -it \
  --rm \
  --link sentineld-agent:agent \
  -v /var/lib/sentineld:/data \
  sentineld/sentinelcli:latest

# Execute specific command
docker run --rm \
  --link sentineld-agent:agent \
  -v /var/lib/sentineld:/data \
  sentineld/sentinelcli:latest query "SELECT * FROM processes LIMIT 10"
```

### Multi-Container Deployment

**Create Network**:

```bash
# Create custom network
docker network create sentineld-network

# Run with custom network
docker run -d \
  --name sentineld-procmond \
  --network sentineld-network \
  --privileged \
  -v /var/lib/sentineld:/data \
  sentineld/procmond:latest

docker run -d \
  --name sentineld-agent \
  --network sentineld-network \
  -v /var/lib/sentineld:/data \
  sentineld/sentinelagent:latest
```

## Docker Compose Deployment

### Basic Docker Compose

**docker-compose.yml**:

```yaml
version: '3.8'

services:
  procmond:
    image: sentineld/procmond:latest
    container_name: sentineld-procmond
    privileged: true
    volumes:
      - /var/lib/sentineld:/data
      - /var/log/sentineld:/logs
      - ./config:/config:ro
    environment:
      - SENTINELD_LOG_LEVEL=info
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network

  sentinelagent:
    image: sentineld/sentinelagent:latest
    container_name: sentineld-agent
    depends_on:
      - procmond
    volumes:
      - /var/lib/sentineld:/data
      - /var/log/sentineld:/logs
      - ./config:/config:ro
    environment:
      - SENTINELD_LOG_LEVEL=info
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network

  sentinelcli:
    image: sentineld/sentinelcli:latest
    container_name: sentineld-cli
    depends_on:
      - sentinelagent
    volumes:
      - /var/lib/sentineld:/data
      - ./config:/config:ro
    environment:
      - SENTINELD_DATA_DIR=/data
    command: [--help]
    restart: no
    networks:
      - sentineld-network

networks:
  sentineld-network:
    driver: bridge

volumes:
  sentineld-data:
    driver: local
  sentineld-logs:
    driver: local
```

### Production Docker Compose

**docker-compose.prod.yml**:

```yaml
version: '3.8'

services:
  procmond:
    image: sentineld/procmond:1.0.0
    container_name: sentineld-procmond
    privileged: true
    user: 1000:1000
    volumes:
      - sentineld-data:/data
      - sentineld-logs:/logs
      - ./config/procmond.yaml:/config/config.yaml:ro
      - ./rules:/rules:ro
    environment:
      - SENTINELD_LOG_LEVEL=info
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
      - SENTINELD_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
    healthcheck:
      test: [CMD, procmond, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  sentinelagent:
    image: sentineld/sentinelagent:1.0.0
    container_name: sentineld-agent
    depends_on:
      procmond:
        condition: service_healthy
    user: 1000:1000
    volumes:
      - sentineld-data:/data
      - sentineld-logs:/logs
      - ./config/sentinelagent.yaml:/config/config.yaml:ro
    environment:
      - SENTINELD_LOG_LEVEL=info
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
      - SENTINELD_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
    healthcheck:
      test: [CMD, sentinelagent, health]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  security-center:
    image: sentineld/security-center:1.0.0
    container_name: sentineld-security-center
    depends_on:
      sentinelagent:
        condition: service_healthy
    user: 1000:1000
    volumes:
      - sentineld-data:/data
      - ./config/security-center.yaml:/config/config.yaml:ro
    environment:
      - SENTINELD_LOG_LEVEL=info
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_AGENT_ENDPOINT=tcp://sentinelagent:8080
      - SENTINELD_WEB_PORT=8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
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
    container_name: sentineld-nginx
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
      - sentineld-network

networks:
  sentineld-network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16

volumes:
  sentineld-data:
    driver: local
  sentineld-logs:
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
    container_name: sentineld-procmond-dev
    privileged: true
    volumes:
      - ./data:/data
      - ./logs:/logs
      - ./config:/config:ro
      - ./rules:/rules:ro
    environment:
      - SENTINELD_LOG_LEVEL=debug
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
      - SENTINELD_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network

  sentinelagent:
    build:
      context: .
      dockerfile: sentinelagent/Dockerfile
    container_name: sentineld-agent-dev
    depends_on:
      - procmond
    volumes:
      - ./data:/data
      - ./logs:/logs
      - ./config:/config:ro
    environment:
      - SENTINELD_LOG_LEVEL=debug
      - SENTINELD_DATA_DIR=/data
      - SENTINELD_LOG_DIR=/logs
      - SENTINELD_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network

  sentinelcli:
    build:
      context: .
      dockerfile: sentinelcli/Dockerfile
    container_name: sentineld-cli-dev
    depends_on:
      - sentinelagent
    volumes:
      - ./data:/data
      - ./config:/config:ro
    environment:
      - SENTINELD_DATA_DIR=/data
    command: [--help]
    restart: no
    networks:
      - sentineld-network

networks:
  sentineld-network:
    driver: bridge
```

## Production Deployment

### Production Configuration

**Environment Variables**:

```bash
# .env file
SENTINELD_VERSION=1.0.0
SENTINELD_LOG_LEVEL=info
SENTINELD_DATA_DIR=/var/lib/sentineld
SENTINELD_LOG_DIR=/var/log/sentineld
SENTINELD_CONFIG_DIR=/etc/sentineld

# Database settings
DATABASE_PATH=/var/lib/sentineld/processes.db
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
    image: sentineld/procmond:${SENTINELD_VERSION}
    container_name: sentineld-procmond
    privileged: true
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - sentineld-data:/data
      - sentineld-logs:/logs
      - ./config/procmond.yaml:/config/config.yaml:ro
      - ./rules:/rules:ro
    environment:
      - SENTINELD_LOG_LEVEL=${SENTINELD_LOG_LEVEL}
      - SENTINELD_DATA_DIR=${SENTINELD_DATA_DIR}
      - SENTINELD_LOG_DIR=${SENTINELD_LOG_DIR}
      - SENTINELD_RULE_DIR=/rules
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
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

  sentinelagent:
    image: sentineld/sentinelagent:${SENTINELD_VERSION}
    container_name: sentineld-agent
    depends_on:
      procmond:
        condition: service_healthy
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - sentineld-data:/data
      - sentineld-logs:/logs
      - ./config/sentinelagent.yaml:/config/config.yaml:ro
    environment:
      - SENTINELD_LOG_LEVEL=${SENTINELD_LOG_LEVEL}
      - SENTINELD_DATA_DIR=${SENTINELD_DATA_DIR}
      - SENTINELD_LOG_DIR=${SENTINELD_LOG_DIR}
      - SENTINELD_PROCMOND_ENDPOINT=tcp://procmond:8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
    healthcheck:
      test: [CMD, sentinelagent, health]
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
    image: sentineld/security-center:${SENTINELD_VERSION}
    container_name: sentineld-security-center
    depends_on:
      sentinelagent:
        condition: service_healthy
    user: ${SECURITY_DROP_TO_USER}:${SECURITY_DROP_TO_GROUP}
    volumes:
      - sentineld-data:/data
      - ./config/security-center.yaml:/config/config.yaml:ro
    environment:
      - SENTINELD_LOG_LEVEL=${SENTINELD_LOG_LEVEL}
      - SENTINELD_DATA_DIR=${SENTINELD_DATA_DIR}
      - SENTINELD_AGENT_ENDPOINT=tcp://sentinelagent:8080
      - SENTINELD_WEB_PORT=8080
    command: [--config, /config/config.yaml]
    restart: unless-stopped
    networks:
      - sentineld-network
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
  sentineld-network:
    driver: bridge
    ipam:
      config:
        - subnet: 172.20.0.0/16

volumes:
  sentineld-data:
    driver: local
  sentineld-logs:
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
BACKUP_DIR="/backup/sentineld"
LOG_DIR="/var/log/sentineld"

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
    if docker volume ls | grep -q sentineld-data; then
        docker run --rm -v sentineld-data:/data -v "$BACKUP_DIR":/backup alpine tar czf /backup/data.tar.gz -C /data .
    fi

    # Backup logs
    if [ -d "$LOG_DIR" ]; then
        cp -r "$LOG_DIR" "$BACKUP_DIR/logs"
    fi

    log_info "Backup completed"
}

# Deploy SentinelD
deploy_sentineld() {
    log_info "Deploying SentinelD..."

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
    log_info "Starting SentinelD deployment..."

    check_prerequisites
    backup_deployment
    deploy_sentineld

    log_info "SentinelD deployment completed successfully"
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
BACKUP_DIR="/backup/sentineld"

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
    log_info "Rolling back SentinelD deployment..."

    # Stop current services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" down

    # Restore data volumes
    if [ -f "$BACKUP_DIR/data.tar.gz" ]; then
        docker run --rm -v sentineld-data:/data -v "$BACKUP_DIR":/backup alpine tar xzf /backup/data.tar.gz -C /data
    fi

    # Restore logs
    if [ -d "$BACKUP_DIR/logs" ]; then
        cp -r "$BACKUP_DIR/logs"/* /var/log/sentineld/
    fi

    # Start services
    docker-compose -f "$COMPOSE_FILE" --env-file "$ENV_FILE" up -d

    log_info "Rollback completed"
}

# Main execution
main() {
    log_info "Starting SentinelD rollback..."

    rollback_deployment

    log_info "SentinelD rollback completed successfully"
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
  name: sentineld
  labels:
    name: sentineld
```

**ConfigMap**:

```yaml
# configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: sentineld-config
  namespace: sentineld
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

  sentinelagent.yaml: |
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
          url: http://sentineld-webhook:8080/webhook
```

**Secret**:

```yaml
# secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: sentineld-secrets
  namespace: sentineld
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
  name: sentineld-data
  namespace: sentineld
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
  name: sentineld-procmond
  namespace: sentineld
spec:
  selector:
    matchLabels:
      app: sentineld-procmond
  template:
    metadata:
      labels:
        app: sentineld-procmond
    spec:
      serviceAccountName: sentineld-procmond
      containers:
        - name: procmond
          image: sentineld/procmond:1.0.0
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
            - name: SENTINELD_LOG_LEVEL
              value: info
            - name: SENTINELD_DATA_DIR
              value: /data
            - name: SENTINELD_LOG_DIR
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
            name: sentineld-config
        - name: data
          persistentVolumeClaim:
            claimName: sentineld-data
        - name: logs
          emptyDir: {}
        - name: rules
          configMap:
            name: sentineld-rules
      tolerations:
        - key: node-role.kubernetes.io/master
          operator: Exists
          effect: NoSchedule
        - key: node-role.kubernetes.io/control-plane
          operator: Exists
          effect: NoSchedule
```

**Deployment for sentinelagent**:

```yaml
# sentinelagent-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sentineld-agent
  namespace: sentineld
spec:
  replicas: 1
  selector:
    matchLabels:
      app: sentineld-agent
  template:
    metadata:
      labels:
        app: sentineld-agent
    spec:
      serviceAccountName: sentineld-agent
      containers:
        - name: sentinelagent
          image: sentineld/sentinelagent:1.0.0
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
            - name: SENTINELD_LOG_LEVEL
              value: info
            - name: SENTINELD_DATA_DIR
              value: /data
            - name: SENTINELD_LOG_DIR
              value: /logs
            - name: SENTINELD_PROCMOND_ENDPOINT
              value: tcp://sentineld-procmond:8080
          command: [sentinelagent]
          args: [--config, /config/sentinelagent.yaml]
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
                - sentinelagent
                - health
            initialDelaySeconds: 30
            periodSeconds: 30
            timeoutSeconds: 10
            failureThreshold: 3
          readinessProbe:
            exec:
              command:
                - sentinelagent
                - health
            initialDelaySeconds: 10
            periodSeconds: 10
            timeoutSeconds: 5
            failureThreshold: 3
      volumes:
        - name: config
          configMap:
            name: sentineld-config
        - name: data
          persistentVolumeClaim:
            claimName: sentineld-data
        - name: logs
          emptyDir: {}
```

**Service**:

```yaml
# service.yaml
apiVersion: v1
kind: Service
metadata:
  name: sentineld-agent
  namespace: sentineld
spec:
  selector:
    app: sentineld-agent
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
  name: sentineld-procmond
  namespace: sentineld
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: sentineld-agent
  namespace: sentineld
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: sentineld-procmond
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
  name: sentineld-procmond
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: sentineld-procmond
subjects:
- kind: ServiceAccount
  name: sentineld-procmond
  namespace: sentineld
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: sentineld-agent
rules:
- apiGroups: [""]
  resources: ["pods", "services", "endpoints"]
  verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: sentineld-agent
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: sentineld-agent
subjects:
- kind: ServiceAccount
  name: sentineld-agent
  namespace: sentineld
```

### Helm Chart

**Chart.yaml**:

```yaml
# Chart.yaml
apiVersion: v2
name: sentineld
description: SentinelD Security Monitoring Agent
type: application
version: 1.0.0
appVersion: 1.0.0
keywords:
  - security
  - monitoring
  - processes
  - threat-detection
home: https://sentineld.com
sources:
  - https://github.com/sentineld/sentineld
maintainers:
  - name: SentinelD Team
    email: team@sentineld.com
```

**values.yaml**:

```yaml
# values.yaml
image:
  repository: sentineld
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
    - host: sentineld.example.com
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
  name: sentineld-network-policy
  namespace: sentineld
spec:
  podSelector:
    matchLabels:
      app: sentineld
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              name: sentineld
      ports:
        - protocol: TCP
          port: 8080
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              name: sentineld
      ports:
        - protocol: TCP
          port: 8080
```

### Image Security

**Image Scanning**:

```bash
# Scan images for vulnerabilities
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  aquasec/trivy image sentineld/procmond:1.0.0

# Scan with specific severity levels
docker run --rm -v /var/run/docker.sock:/var/run/docker.sock \
  aquasec/trivy image --severity HIGH,CRITICAL sentineld/procmond:1.0.0
```

**Image Signing**:

```bash
# Sign images with Docker Content Trust
export DOCKER_CONTENT_TRUST=1
docker push sentineld/procmond:1.0.0
```

**Multi-stage Builds**:

```dockerfile
# Multi-stage build for smaller attack surface
FROM rust:1.85-alpine AS builder
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
    "title": "SentinelD Monitoring",
    "panels": [
      {
        "title": "Process Collection Rate",
        "type": "graph",
        "targets": [
          {
            "expr": "rate(sentineld_processes_collected_total[5m])",
            "legendFormat": "Processes/sec"
          }
        ]
      },
      {
        "title": "Memory Usage",
        "type": "graph",
        "targets": [
          {
            "expr": "sentineld_memory_usage_bytes",
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
  namespace: sentineld
data:
  fluent.conf: |
    <source>
      @type tail
      path /var/log/sentineld/*.log
      pos_file /var/log/fluentd/sentineld.log.pos
      tag sentineld.*
      format json
    </source>

    <match sentineld.**>
      @type elasticsearch
      host elasticsearch.logging.svc.cluster.local
      port 9200
      index_name sentineld
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
docker logs sentineld-procmond

# Check container status
docker ps -a

# Check resource usage
docker stats sentineld-procmond
```

**Permission Denied**:

```bash
# Check file permissions
docker exec sentineld-procmond ls -la /data

# Fix permissions
docker exec sentineld-procmond chown -R 1000:1000 /data
```

**Network Issues**:

```bash
# Check network connectivity
docker exec sentineld-agent ping sentineld-procmond

# Check DNS resolution
docker exec sentineld-agent nslookup sentineld-procmond
```

**Database Issues**:

```bash
# Check database status
docker exec sentineld-agent sentinelcli database status

# Check database integrity
docker exec sentineld-agent sentinelcli database integrity-check

# Repair database
docker exec sentineld-agent sentinelcli database repair
```

### Debug Mode

**Enable Debug Logging**:

```yaml
# docker-compose.yml
services:
  procmond:
    environment:
      - SENTINELD_LOG_LEVEL=debug
    command: [procmond, --config, /config/config.yaml, --log-level, debug]
```

**Debug Container**:

```bash
# Run debug container
docker run -it --rm --privileged \
  -v /var/lib/sentineld:/data \
  -v /var/log/sentineld:/logs \
  sentineld/procmond:latest /bin/sh

# Check system capabilities
docker run --rm --privileged sentineld/procmond:latest capsh --print
```

### Performance Issues

**High CPU Usage**:

```bash
# Check CPU usage
docker stats sentineld-procmond

# Reduce scan frequency
docker exec sentineld-procmond sentinelcli config set app.scan_interval_ms 120000
```

**High Memory Usage**:

```bash
# Check memory usage
docker stats sentineld-procmond

# Limit memory usage
docker run -d --memory=512m sentineld/procmond:latest
```

**Slow Database Operations**:

```bash
# Check database performance
docker exec sentineld-agent sentinelcli database query-stats

# Optimize database
docker exec sentineld-agent sentinelcli database optimize
```

### Health Checks

**Container Health**:

```bash
# Check container health
docker inspect sentineld-procmond | jq '.[0].State.Health'

# Run health check manually
docker exec sentineld-procmond procmond health
```

**Service Health**:

```bash
# Check service health
curl http://localhost:8080/health

# Check metrics
curl http://localhost:9090/metrics
```

---

*This Docker deployment guide provides comprehensive instructions for containerizing and deploying SentinelD. For additional help, consult the troubleshooting section or contact support.*
