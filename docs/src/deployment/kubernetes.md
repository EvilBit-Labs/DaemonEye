# DaemonEye Kubernetes Deployment Guide

This guide provides comprehensive instructions for deploying DaemonEye on Kubernetes, including manifests, Helm charts, and production deployment strategies.

---

## Table of Contents

[TOC]

---

## Kubernetes Overview

DaemonEye is designed to run efficiently on Kubernetes, providing:

- **Scalability**: Horizontal pod autoscaling and cluster-wide deployment
- **High Availability**: Multi-replica deployments with health checks
- **Security**: RBAC, network policies, and pod security standards
- **Observability**: Prometheus metrics, structured logging, and distributed tracing
- **Management**: Helm charts and GitOps integration

### Architecture Components

- **procmond**: DaemonSet for process monitoring on each node
- **daemoneye-agent**: Deployment for alerting and orchestration
- **daemoneye-cli**: Job/CronJob for management tasks
- **Security Center**: Deployment for web-based management (Business/Enterprise)

## Prerequisites

### Cluster Requirements

**Minimum Requirements**:

- Kubernetes 1.20+
- 2+ worker nodes
- 4+ CPU cores total
- 8+ GB RAM total
- 50+ GB storage

**Recommended Requirements**:

- Kubernetes 1.24+
- 3+ worker nodes
- 8+ CPU cores total
- 16+ GB RAM total
- 100+ GB storage

### Required Tools

```bash
# Install kubectl
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
sudo install -o root -g root -m 0755 kubectl /usr/local/bin/kubectl

# Install Helm
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# Install kustomize
curl -s "https://raw.githubusercontent.com/kubernetes-sigs/kustomize/master/hack/install_kustomize.sh" | bash
```

## Basic Deployment

### Namespace and RBAC

**namespace.yaml**:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: daemoneye
  labels:
    name: daemoneye
    app.kubernetes.io/name: daemoneye
    app.kubernetes.io/version: 1.0.0
```

**rbac.yaml**:

```yaml
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
```

### ConfigMap and Secrets

**configmap.yaml**:

```yaml
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

**secret.yaml**:

```yaml
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

### Persistent Storage

**pvc.yaml**:

```yaml
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

### DaemonSet for procmond

**procmond-daemonset.yaml**:

```yaml
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
      tolerations:
        - key: node-role.kubernetes.io/master
          operator: Exists
          effect: NoSchedule
        - key: node-role.kubernetes.io/control-plane
          operator: Exists
          effect: NoSchedule
```

### Deployment for daemoneye-agent

**daemoneye-agent-deployment.yaml**:

```yaml
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

### Service

**service.yaml**:

```yaml
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

### Deploy Basic Setup

```bash
# Create namespace
kubectl apply -f namespace.yaml

# Apply RBAC
kubectl apply -f rbac.yaml

# Apply configuration
kubectl apply -f configmap.yaml
kubectl apply -f secret.yaml

# Apply storage
kubectl apply -f pvc.yaml

# Deploy components
kubectl apply -f procmond-daemonset.yaml
kubectl apply -f daemoneye-agent-deployment.yaml
kubectl apply -f service.yaml

# Check deployment status
kubectl get pods -n daemoneye
kubectl get services -n daemoneye
```

## Production Deployment

### Production Configuration

**production-configmap.yaml**:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: daemoneye-config
  namespace: daemoneye
data:
  procmond.yaml: |
    app:
      scan_interval_ms: 60000
      batch_size: 1000
      log_level: info
      data_dir: /data
      log_dir: /logs
      max_memory_mb: 512
      max_cpu_percent: 5.0
    database:
      path: /data/processes.db
      retention_days: 30
      max_connections: 20
      cache_size: -128000
      wal_mode: true
    security:
      enable_privilege_dropping: true
      drop_to_user: 1000
      drop_to_group: 1000
      enable_audit_logging: true
      audit_log_path: /logs/audit.log

  daemoneye-agent.yaml: |
    app:
      scan_interval_ms: 60000
      batch_size: 1000
      log_level: info
      data_dir: /data
      log_dir: /logs
      max_memory_mb: 1024
      max_cpu_percent: 10.0
    database:
      path: /data/processes.db
      retention_days: 30
      max_connections: 20
      cache_size: -128000
      wal_mode: true
    alerting:
      enabled: true
      max_queue_size: 10000
      delivery_timeout_ms: 5000
      retry_attempts: 3
      sinks:
        - type: syslog
          enabled: true
          facility: daemon
          priority: info
        - type: webhook
          enabled: true
          url: http://daemoneye-webhook:8080/webhook
          timeout_ms: 5000
          retry_attempts: 3
        - type: file
          enabled: true
          path: /logs/alerts.log
          format: json
          rotation: daily
          max_files: 30
    detection:
      enable_detection: true
      rule_directory: /rules
      enable_hot_reload: true
      max_concurrent_rules: 10
      rule_timeout_ms: 30000
      enable_rule_caching: true
      cache_ttl_seconds: 300
    observability:
      enable_metrics: true
      metrics_port: 9090
      metrics_path: /metrics
      enable_health_checks: true
      health_check_port: 8080
      health_check_path: /health
      logging:
        enable_structured_logging: true
        log_format: json
        enable_log_rotation: true
        max_log_file_size_mb: 100
        max_log_files: 10
```

### Production DaemonSet

**production-procmond-daemonset.yaml**:

```yaml
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
      annotations:
        prometheus.io/scrape: 'true'
        prometheus.io/port: '9090'
        prometheus.io/path: /metrics
    spec:
      serviceAccountName: daemoneye-procmond
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
      containers:
        - name: procmond
          image: daemoneye/procmond:1.0.0
          imagePullPolicy: IfNotPresent
          securityContext:
            privileged: true
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            runAsUser: 1000
            runAsGroup: 1000
            capabilities:
              add:
                - CAP_SYS_PTRACE
                - CAP_SYS_ADMIN
              drop:
                - ALL
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
            - name: tmp
              mountPath: /tmp
          env:
            - name: DaemonEye_LOG_LEVEL
              value: info
            - name: DaemonEye_DATA_DIR
              value: /data
            - name: DaemonEye_LOG_DIR
              value: /logs
            - name: DaemonEye_RULE_DIR
              value: /rules
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
          ports:
            - name: metrics
              containerPort: 9090
              protocol: TCP
            - name: health
              containerPort: 8080
              protocol: TCP
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
        - name: tmp
          emptyDir: {}
      tolerations:
        - key: node-role.kubernetes.io/master
          operator: Exists
          effect: NoSchedule
        - key: node-role.kubernetes.io/control-plane
          operator: Exists
          effect: NoSchedule
        - key: node.kubernetes.io/not-ready
          operator: Exists
          effect: NoExecute
          tolerationSeconds: 300
        - key: node.kubernetes.io/unreachable
          operator: Exists
          effect: NoExecute
          tolerationSeconds: 300
      nodeSelector:
        kubernetes.io/os: linux
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
              - matchExpressions:
                  - key: kubernetes.io/arch
                    operator: In
                    values:
                      - amd64
                      - arm64
```

### Production Deployment

**production-daemoneye-agent-deployment.yaml**:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: daemoneye-agent
  namespace: daemoneye
spec:
  replicas: 2
  selector:
    matchLabels:
      app: daemoneye-agent
  template:
    metadata:
      labels:
        app: daemoneye-agent
      annotations:
        prometheus.io/scrape: 'true'
        prometheus.io/port: '9090'
        prometheus.io/path: /metrics
    spec:
      serviceAccountName: daemoneye-agent
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
      containers:
        - name: daemoneye-agent
          image: daemoneye/daemoneye-agent:1.0.0
          imagePullPolicy: IfNotPresent
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            runAsUser: 1000
            runAsGroup: 1000
            capabilities:
              drop:
                - ALL
          volumeMounts:
            - name: config
              mountPath: /config
              readOnly: true
            - name: data
              mountPath: /data
            - name: logs
              mountPath: /logs
            - name: tmp
              mountPath: /tmp
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
          ports:
            - name: metrics
              containerPort: 9090
              protocol: TCP
            - name: health
              containerPort: 8080
              protocol: TCP
      volumes:
        - name: config
          configMap:
            name: daemoneye-config
        - name: data
          persistentVolumeClaim:
            claimName: daemoneye-data
        - name: logs
          emptyDir: {}
        - name: tmp
          emptyDir: {}
      affinity:
        podAntiAffinity:
          preferredDuringSchedulingIgnoredDuringExecution:
            - weight: 100
              podAffinityTerm:
                labelSelector:
                  matchExpressions:
                    - key: app
                      operator: In
                      values:
                        - daemoneye-agent
                topologyKey: kubernetes.io/hostname
```

### Horizontal Pod Autoscaler

**hpa.yaml**:

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: daemoneye-agent-hpa
  namespace: daemoneye
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: daemoneye-agent
  minReplicas: 2
  maxReplicas: 10
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
    - type: Resource
      resource:
        name: memory
        target:
          type: Utilization
          averageUtilization: 80
  behavior:
    scaleDown:
      stabilizationWindowSeconds: 300
      policies:
        - type: Percent
          value: 10
          periodSeconds: 60
    scaleUp:
      stabilizationWindowSeconds: 60
      policies:
        - type: Percent
          value: 50
          periodSeconds: 60
```

## Helm Chart Deployment

### Helm Chart Structure

```
daemoneye/
├── Chart.yaml
├── values.yaml
├── values-production.yaml
├── values-development.yaml
├── templates/
│   ├── namespace.yaml
│   ├── rbac.yaml
│   ├── configmap.yaml
│   ├── secret.yaml
│   ├── pvc.yaml
│   ├── procmond-daemonset.yaml
│   ├── daemoneye-agent-deployment.yaml
│   ├── service.yaml
│   ├── hpa.yaml
│   ├── networkpolicy.yaml
│   └── servicemonitor.yaml
└── charts/
```

### Chart.yaml

```yaml
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
dependencies:
  - name: prometheus
    version: 15.0.0
    repository: https://prometheus-community.github.io/helm-charts
    condition: monitoring.prometheus.enabled
```

### values.yaml

```yaml
# Default values for daemoneye
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
  prometheus:
    enabled: false
    server:
      enabled: true
      persistentVolume:
        enabled: true
        size: 8Gi
    alertmanager:
      enabled: true
      persistentVolume:
        enabled: true
        size: 2Gi
  grafana:
    enabled: false
    adminPassword: admin
    persistentVolume:
      enabled: true
      size: 1Gi

networkPolicy:
  enabled: false
  ingress:
    enabled: true
    rules: []
  egress:
    enabled: true
    rules: []
```

### Deploy with Helm

```bash
# Add DaemonEye Helm repository
helm repo add daemoneye https://charts.daemoneye.com
helm repo update

# Install DaemonEye
helm install daemoneye daemoneye/daemoneye \
  --namespace daemoneye \
  --create-namespace \
  --values values.yaml

# Install with production values
helm install daemoneye daemoneye/daemoneye \
  --namespace daemoneye \
  --create-namespace \
  --values values-production.yaml

# Upgrade deployment
helm upgrade daemoneye daemoneye/daemoneye \
  --namespace daemoneye \
  --values values.yaml

# Uninstall
helm uninstall daemoneye --namespace daemoneye
```

## Security Configuration

### Network Policies

**networkpolicy.yaml**:

```yaml
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
        - podSelector:
            matchLabels:
              app: daemoneye
      ports:
        - protocol: TCP
          port: 8080
        - protocol: TCP
          port: 9090
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              name: daemoneye
        - podSelector:
            matchLabels:
              app: daemoneye
      ports:
        - protocol: TCP
          port: 8080
        - protocol: TCP
          port: 9090
    - to: []
      ports:
        - protocol: TCP
          port: 53
        - protocol: UDP
          port: 53
```

### Pod Security Standards

**pod-security-policy.yaml**:

```yaml
apiVersion: policy/v1beta1
kind: PodSecurityPolicy
metadata:
  name: daemoneye-psp
spec:
  privileged: false
  allowPrivilegeEscalation: false
  requiredDropCapabilities:
    - ALL
  volumes:
    - configMap
    - emptyDir
    - projected
    - secret
    - downwardAPI
    - persistentVolumeClaim
  runAsUser:
    rule: MustRunAsNonRoot
  seLinux:
    rule: RunAsAny
  fsGroup:
    rule: RunAsAny
```

### RBAC Configuration

**rbac.yaml**:

```yaml
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

## Monitoring and Observability

### Prometheus ServiceMonitor

**servicemonitor.yaml**:

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: daemoneye
  namespace: daemoneye
  labels:
    app: daemoneye
spec:
  selector:
    matchLabels:
      app: daemoneye
  endpoints:
    - port: metrics
      path: /metrics
      interval: 30s
      scrapeTimeout: 10s
```

### Grafana Dashboard

**grafana-dashboard.yaml**:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: daemoneye-grafana-dashboard
  namespace: daemoneye
  labels:
    grafana_dashboard: '1'
data:
  daemoneye-dashboard.json: |
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

## Troubleshooting

### Common Issues

**Pod Won't Start**:

```bash
# Check pod status
kubectl get pods -n daemoneye

# Check pod logs
kubectl logs -n daemoneye daemoneye-procmond-xxx

# Check pod events
kubectl describe pod -n daemoneye daemoneye-procmond-xxx
```

**Permission Denied**:

```bash
# Check security context
kubectl get pod -n daemoneye daemoneye-procmond-xxx -o yaml | grep securityContext

# Check file permissions
kubectl exec -n daemoneye daemoneye-procmond-xxx -- ls -la /data
```

**Network Issues**:

```bash
# Check service endpoints
kubectl get endpoints -n daemoneye

# Check network connectivity
kubectl exec -n daemoneye daemoneye-agent-xxx -- ping daemoneye-procmond
```

**Database Issues**:

```bash
# Check database status
kubectl exec -n daemoneye daemoneye-agent-xxx -- daemoneye-cli database status

# Check database integrity
kubectl exec -n daemoneye daemoneye-agent-xxx -- daemoneye-cli database integrity-check
```

### Debug Mode

**Enable Debug Logging**:

```yaml
# Update ConfigMap
apiVersion: v1
kind: ConfigMap
metadata:
  name: daemoneye-config
  namespace: daemoneye
data:
  procmond.yaml: |
    app:
      log_level: debug
    # ... rest of config
```

**Debug Pod**:

```bash
# Run debug pod
kubectl run debug --image=daemoneye/daemoneye-cli:1.0.0 -it --rm -- /bin/sh

# Check system capabilities
kubectl run debug --image=daemoneye/daemoneye-cli:1.0.0 -it --rm -- capsh --print
```

### Performance Issues

**High CPU Usage**:

```bash
# Check resource usage
kubectl top pods -n daemoneye

# Check HPA status
kubectl get hpa -n daemoneye

# Scale up manually
kubectl scale deployment daemoneye-agent --replicas=3 -n daemoneye
```

**High Memory Usage**:

```bash
# Check memory usage
kubectl top pods -n daemoneye

# Check memory limits
kubectl describe pod -n daemoneye daemoneye-agent-xxx | grep Limits
```

**Slow Database Operations**:

```bash
# Check database performance
kubectl exec -n daemoneye daemoneye-agent-xxx -- daemoneye-cli database query-stats

# Optimize database
kubectl exec -n daemoneye daemoneye-agent-xxx -- daemoneye-cli database optimize
```

---

*This Kubernetes deployment guide provides comprehensive instructions for deploying DaemonEye on Kubernetes. For additional help, consult the troubleshooting section or contact support.*
