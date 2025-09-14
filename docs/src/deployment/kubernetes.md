# SentinelD Kubernetes Deployment Guide

This guide provides comprehensive instructions for deploying SentinelD on Kubernetes, including manifests, Helm charts, and production deployment strategies.

---

## Table of Contents

[TOC]

---

## Kubernetes Overview

SentinelD is designed to run efficiently on Kubernetes, providing:

- **Scalability**: Horizontal pod autoscaling and cluster-wide deployment
- **High Availability**: Multi-replica deployments with health checks
- **Security**: RBAC, network policies, and pod security standards
- **Observability**: Prometheus metrics, structured logging, and distributed tracing
- **Management**: Helm charts and GitOps integration

### Architecture Components

- **procmond**: DaemonSet for process monitoring on each node
- **sentinelagent**: Deployment for alerting and orchestration
- **sentinelcli**: Job/CronJob for management tasks
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
  name: sentineld
  labels:
    name: sentineld
    app.kubernetes.io/name: sentineld
    app.kubernetes.io/version: 1.0.0
```

**rbac.yaml**:

```yaml
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
```

### ConfigMap and Secrets

**configmap.yaml**:

```yaml
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

**secret.yaml**:

```yaml
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

### Persistent Storage

**pvc.yaml**:

```yaml
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

### DaemonSet for procmond

**procmond-daemonset.yaml**:

```yaml
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
      tolerations:
        - key: node-role.kubernetes.io/master
          operator: Exists
          effect: NoSchedule
        - key: node-role.kubernetes.io/control-plane
          operator: Exists
          effect: NoSchedule
```

### Deployment for sentinelagent

**sentinelagent-deployment.yaml**:

```yaml
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

### Service

**service.yaml**:

```yaml
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
kubectl apply -f sentinelagent-deployment.yaml
kubectl apply -f service.yaml

# Check deployment status
kubectl get pods -n sentineld
kubectl get services -n sentineld
```

## Production Deployment

### Production Configuration

**production-configmap.yaml**:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: sentineld-config
  namespace: sentineld
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

  sentinelagent.yaml: |
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
          url: http://sentineld-webhook:8080/webhook
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
      annotations:
        prometheus.io/scrape: 'true'
        prometheus.io/port: '9090'
        prometheus.io/path: /metrics
    spec:
      serviceAccountName: sentineld-procmond
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
      containers:
        - name: procmond
          image: sentineld/procmond:1.0.0
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
            - name: SENTINELD_LOG_LEVEL
              value: info
            - name: SENTINELD_DATA_DIR
              value: /data
            - name: SENTINELD_LOG_DIR
              value: /logs
            - name: SENTINELD_RULE_DIR
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
            name: sentineld-config
        - name: data
          persistentVolumeClaim:
            claimName: sentineld-data
        - name: logs
          emptyDir: {}
        - name: rules
          configMap:
            name: sentineld-rules
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

**production-sentinelagent-deployment.yaml**:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sentineld-agent
  namespace: sentineld
spec:
  replicas: 2
  selector:
    matchLabels:
      app: sentineld-agent
  template:
    metadata:
      labels:
        app: sentineld-agent
      annotations:
        prometheus.io/scrape: 'true'
        prometheus.io/port: '9090'
        prometheus.io/path: /metrics
    spec:
      serviceAccountName: sentineld-agent
      securityContext:
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
      containers:
        - name: sentinelagent
          image: sentineld/sentinelagent:1.0.0
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
            name: sentineld-config
        - name: data
          persistentVolumeClaim:
            claimName: sentineld-data
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
                        - sentineld-agent
                topologyKey: kubernetes.io/hostname
```

### Horizontal Pod Autoscaler

**hpa.yaml**:

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: sentineld-agent-hpa
  namespace: sentineld
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: sentineld-agent
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
sentineld/
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
│   ├── sentinelagent-deployment.yaml
│   ├── service.yaml
│   ├── hpa.yaml
│   ├── networkpolicy.yaml
│   └── servicemonitor.yaml
└── charts/
```

### Chart.yaml

```yaml
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
dependencies:
  - name: prometheus
    version: 15.0.0
    repository: https://prometheus-community.github.io/helm-charts
    condition: monitoring.prometheus.enabled
```

### values.yaml

```yaml
# Default values for sentineld
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
# Add SentinelD Helm repository
helm repo add sentineld https://charts.sentineld.com
helm repo update

# Install SentinelD
helm install sentineld sentineld/sentineld \
  --namespace sentineld \
  --create-namespace \
  --values values.yaml

# Install with production values
helm install sentineld sentineld/sentineld \
  --namespace sentineld \
  --create-namespace \
  --values values-production.yaml

# Upgrade deployment
helm upgrade sentineld sentineld/sentineld \
  --namespace sentineld \
  --values values.yaml

# Uninstall
helm uninstall sentineld --namespace sentineld
```

## Security Configuration

### Network Policies

**networkpolicy.yaml**:

```yaml
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
        - podSelector:
            matchLabels:
              app: sentineld
      ports:
        - protocol: TCP
          port: 8080
        - protocol: TCP
          port: 9090
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              name: sentineld
        - podSelector:
            matchLabels:
              app: sentineld
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
  name: sentineld-psp
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

## Monitoring and Observability

### Prometheus ServiceMonitor

**servicemonitor.yaml**:

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: sentineld
  namespace: sentineld
  labels:
    app: sentineld
spec:
  selector:
    matchLabels:
      app: sentineld
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
  name: sentineld-grafana-dashboard
  namespace: sentineld
  labels:
    grafana_dashboard: '1'
data:
  sentineld-dashboard.json: |
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

## Troubleshooting

### Common Issues

**Pod Won't Start**:

```bash
# Check pod status
kubectl get pods -n sentineld

# Check pod logs
kubectl logs -n sentineld sentineld-procmond-xxx

# Check pod events
kubectl describe pod -n sentineld sentineld-procmond-xxx
```

**Permission Denied**:

```bash
# Check security context
kubectl get pod -n sentineld sentineld-procmond-xxx -o yaml | grep securityContext

# Check file permissions
kubectl exec -n sentineld sentineld-procmond-xxx -- ls -la /data
```

**Network Issues**:

```bash
# Check service endpoints
kubectl get endpoints -n sentineld

# Check network connectivity
kubectl exec -n sentineld sentineld-agent-xxx -- ping sentineld-procmond
```

**Database Issues**:

```bash
# Check database status
kubectl exec -n sentineld sentineld-agent-xxx -- sentinelcli database status

# Check database integrity
kubectl exec -n sentineld sentineld-agent-xxx -- sentinelcli database integrity-check
```

### Debug Mode

**Enable Debug Logging**:

```yaml
# Update ConfigMap
apiVersion: v1
kind: ConfigMap
metadata:
  name: sentineld-config
  namespace: sentineld
data:
  procmond.yaml: |
    app:
      log_level: debug
    # ... rest of config
```

**Debug Pod**:

```bash
# Run debug pod
kubectl run debug --image=sentineld/sentinelcli:1.0.0 -it --rm -- /bin/sh

# Check system capabilities
kubectl run debug --image=sentineld/sentinelcli:1.0.0 -it --rm -- capsh --print
```

### Performance Issues

**High CPU Usage**:

```bash
# Check resource usage
kubectl top pods -n sentineld

# Check HPA status
kubectl get hpa -n sentineld

# Scale up manually
kubectl scale deployment sentineld-agent --replicas=3 -n sentineld
```

**High Memory Usage**:

```bash
# Check memory usage
kubectl top pods -n sentineld

# Check memory limits
kubectl describe pod -n sentineld sentineld-agent-xxx | grep Limits
```

**Slow Database Operations**:

```bash
# Check database performance
kubectl exec -n sentineld sentineld-agent-xxx -- sentinelcli database query-stats

# Optimize database
kubectl exec -n sentineld sentineld-agent-xxx -- sentinelcli database optimize
```

---

*This Kubernetes deployment guide provides comprehensive instructions for deploying SentinelD on Kubernetes. For additional help, consult the troubleshooting section or contact support.*
