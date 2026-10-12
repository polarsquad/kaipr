# kaipr toolbox image (issue #104).
#
# Ships kaipr-bootstrap plus the full pinned tool layer so a host needs only
# a container engine to run the whole bootstrap lifecycle. Tool versions
# are single-sourced from the committed mise configs (Renovate-managed
# there, never duplicated into this file); the CLI is compiled with the
# toolchain pinned in bootstrap-rs/rust-toolchain.toml.
#
# Build (from the repository root; the context is the repo root):
#   docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
#
# Base image digests and the mise CLI pin are Renovate-managed
# (dockerfile manager with pinDigests, and the jdx/mise custom manager
# extended to this file's ARG MISE_VERSION line).

# ── Stage 1: compile kaipr-bootstrap with the pinned toolchain ──────────────
# slim-bookworm matches the runtime stage's glibc (bookworm, 2.36): a
# binary built on the default rust image (trixie, glibc 2.39) fails on
# bookworm with "GLIBC_2.39 not found".
FROM rust:1.99.0-slim-bookworm@sha256:452176c0cefca88c0b3184ce85a4eb03e3d4fa05d2afb5366abcba853221019e AS build

WORKDIR /build
COPY bootstrap-rs/ ./bootstrap-rs/
RUN cd bootstrap-rs && cargo build --locked --release \
    && cp target/release/kaipr-bootstrap /usr/local/bin/kaipr-bootstrap

# ── Stage 2: runtime with the pinned tool layer ───────────────────────────
FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251

# renovate: datasource=github-releases depName=jdx/mise extractVersion=^v(?<version>.+) versioning=semver
ARG MISE_VERSION=2026.10.3
# Set by BuildKit: "amd64" or "arm64".
ARG TARGETARCH

# docker-ce-cli (client only) comes from Docker's apt repo: bookworm has no
# standalone docker-cli package, and docker.io would drag in the whole
# engine. The client floats with the repo (client/server API is backward
# compatible; the daemon always runs on the host). The podman remote client
# (static build) provides the podman provider path kind and kaipr-bootstrap
# exec (CONTAINER_ENGINE=podman); it talks to the host socket the run
# invocation mounts.
# renovate: datasource=github-releases depName=containers/podman extractVersion=^v(?<version>.+) versioning=semver
ARG PODMAN_VERSION=6.1.3
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git jq findutils gnupg python-is-python3 \
    && install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/debian/gpg \
         -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian bookworm stable" \
         > /etc/apt/sources.list.d/docker.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends docker-ce-cli \
    && rm -rf /var/lib/apt/lists/* \
    && case "${TARGETARCH}" in \
         amd64) MISE_ASSET=mise-v${MISE_VERSION}-linux-x64 ;; \
         arm64) MISE_ASSET=mise-v${MISE_VERSION}-linux-arm64 ;; \
         *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://github.com/jdx/mise/releases/download/v${MISE_VERSION}/${MISE_ASSET}" \
         -o /usr/local/bin/mise \
    && chmod +x /usr/local/bin/mise \
    && curl -fsSL "https://github.com/containers/podman/releases/download/v${PODMAN_VERSION}/podman-remote-static-linux_${TARGETARCH}.tar.gz" \
         | tar -xz -C /tmp --strip-components=1 "bin/podman-remote-static-linux_${TARGETARCH}" \
    && mv "/tmp/podman-remote-static-linux_${TARGETARCH}" /usr/local/bin/podman \
    && chmod +x /usr/local/bin/podman

# The repository is mounted at /workspace at run time; mise runs from the
# working directory with the env layer of the mounted repo (the local-host
# layer is tasks only, so the base pins below cover every tool the lifecycle
# invokes).
ENV MISE_DATA_DIR=/usr/local/share/mise \
    MISE_YES=1 \
    PATH=/usr/local/share/mise/shims:/usr/local/bin:/usr/bin:/sbin:/bin

COPY --from=build /usr/local/bin/kaipr-bootstrap /usr/local/bin/kaipr-bootstrap
COPY bootstrap-rs/toolbox-entrypoint.sh /usr/local/bin/toolbox-entrypoint.sh
COPY mise.toml /opt/kaipr/

RUN chmod +x /usr/local/bin/toolbox-entrypoint.sh

# Install every tool the configs pin except the dev-only toolchains the
# lifecycle never invokes (go, zarf: they resolve from the mounted repo at
# run time, and a dev-toolchain auto-install would slow or break the build).
# `mise x` is avoided: it auto-installs every tool in the env (including the
# dev-only ones) and a failure would abort the build.
RUN cd /opt/kaipr \
    && mise install -y kind helm kubectl clusterctl flux2 sops age \
    && mise install -y python uv \
    && mise reshim

WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/toolbox-entrypoint.sh"]
