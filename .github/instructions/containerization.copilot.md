---
applyTo: '**/Dockerfile,**/Dockerfile.*,**/*.dockerfile,**/docker-compose*.yml,**/docker-compose*.yaml'
description: Concise, actionable instructions for GitHub Copilot to generate secure, efficient, and maintainable Dockerfiles and container configs.
---

# Copilot Containerization Instructions

## Purpose

These instructions are optimized for Copilot to generate secure, efficient, and maintainable Dockerfiles and container configs. Use concise, actionable steps and checklists for every file you create or review.

---

## Core Principles (Always Apply)

1. **Immutability**: Never mutate running containers. Always build new images for code/config changes. Use semantic versioning for tags.
2. **Portability**: Externalize all config via environment variables. Avoid hardcoded values. Use multi-platform base images if needed.
3. **Isolation**: Run a single process per container. Use container networking. Prefer named volumes for persistence.
4. **Efficiency**: Minimize image size. Remove dev tools from production images. Use multi-stage builds by default.

---

## Dockerfile Generation Checklist

- [ ] Use multi-stage builds for all compiled or dependency-heavy languages
- [ ] Select minimal, versioned base images (e.g., `alpine`, `slim`, never `latest` in prod)
- [ ] Optimize layers: combine `RUN` commands, clean up in same layer
- [ ] Create and maintain a comprehensive `.dockerignore` (exclude VCS, dev files, build artifacts, secrets)
- [ ] Use specific, minimal `COPY` instructions (copy only what is needed, in correct order for caching)
- [ ] Define a non-root `USER` for the running application (create dedicated user/group, set permissions)
- [ ] Use `EXPOSE` to document listening ports
- [ ] Use exec form for `CMD` and/or `ENTRYPOINT` (prefer `CMD` for simple apps)
- [ ] Handle all sensitive config via environment variables (never hardcode secrets)
- [ ] Add a `HEALTHCHECK` instruction for liveness/readiness
- [ ] Never include secrets or sensitive data in image layers
- [ ] Integrate static analysis tools (Hadolint, Trivy) in CI

---

## Security Best Practices

- Always run as non-root (define dedicated user/group)
- Use minimal base images (`alpine`, `slim`, `distroless`)
- Scan Dockerfiles and images for vulnerabilities (Hadolint, Trivy)
- Sign images for production (Cosign, Notary)
- Drop unnecessary Linux capabilities; use read-only filesystems when possible
- Never include secrets in image layers; use runtime secrets management

---

## Runtime & Orchestration

- Set CPU/memory limits for all containers
- Use structured logging to STDOUT/STDERR
- Use persistent volumes for stateful data
- Create custom networks for service isolation
- Prefer Kubernetes for large-scale orchestration

---

## Example: Secure Multi-Stage Dockerfile

```dockerfile
# Stage 1: Build
FROM rust:1.85-alpine AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release

# Stage 2: Production
FROM alpine:3.19
WORKDIR /app
COPY --from=build /app/target/release/myapp ./myapp
RUN addgroup -S appgroup && adduser -S appuser -G appgroup \
    && chown appuser:appgroup /app/myapp
USER appuser
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ./myapp --health || exit 1
CMD ["./myapp"]
```

---

## Example: .dockerignore

```dockerignore
.git*
target
node_modules
__pycache__
*.log
*.env*
docs/
test/
tests/
spec/
__tests__/
.DS_Store
```

---

## CI Integration Example

```yaml
  - name: Lint Dockerfile
    run: docker run --rm -i hadolint/hadolint < Dockerfile
  - name: Scan image
    run: |
      docker build -t myapp .
      trivy image myapp
```

---

## Troubleshooting Quick Reference

- Large image: Use `docker history <image>`, multi-stage builds, minimal base
- Slow builds: Optimize layer order, use `.dockerignore`
- Container crash: Check `CMD`/`ENTRYPOINT`, logs, dependencies
- Permissions: Verify file/user permissions, volume mounts
- Network: Check `EXPOSE`, published ports, network config

---

## Summary

Always apply these checklists and patterns when generating or reviewing Dockerfiles and container configs. Prioritize security, efficiency, and reproducibility. Keep instructions concise and actionable for automation.
