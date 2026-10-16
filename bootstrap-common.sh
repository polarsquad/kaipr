#!/usr/bin/env bash
# bootstrap-common.sh – Shared preflight and Flux-seeding helpers used by
# bootstrap.sh. Sourced, never executed directly.
#
# Environment variables set by these helpers are plain globals in the
# sourcing shell (bash 3.2 compatible: no namerefs, no associative arrays).

# ── Preflight: container engine ───────────────────────────────────────────────
# Detects and selects a running container engine. Sets: CONTAINER_ENGINE,
# ENGINE_SOCK (and exports KIND_EXPERIMENTAL_PROVIDER for podman).
# Note: when using podman via the docker CLI shim (e.g., on macOS),
# `docker --version` reports podman; we check for that case first.
detect_container_engine() {
  if [ -z "${CONTAINER_ENGINE:-}" ]; then
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
      if docker --version 2>/dev/null | grep -qi podman; then
        CONTAINER_ENGINE=podman
      else
        CONTAINER_ENGINE=docker
      fi
    elif command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
      CONTAINER_ENGINE=podman
    else
      echo "ERROR: No running container engine found (tried docker and podman)" >&2
      exit 1
    fi
  fi

  case "$CONTAINER_ENGINE" in
    docker)
      docker info >/dev/null 2>&1 || { echo "ERROR: Docker daemon not running" >&2; exit 1; }
      ENGINE_SOCK="/var/run/docker.sock"
      ;;
    podman)
      podman info >/dev/null 2>&1 || { echo "ERROR: Podman is not running (is 'podman machine' started?)" >&2; exit 1; }
      export KIND_EXPERIMENTAL_PROVIDER=podman
      ENGINE_SOCK="$(podman info --format '{{.Host.RemoteSocket.Path}}' 2>/dev/null || true)"
      ENGINE_SOCK="${ENGINE_SOCK#unix://}"
      if [ -z "$ENGINE_SOCK" ]; then
        ENGINE_SOCK="/run/podman/podman.sock"
        echo ">>> WARNING: Could not detect the podman API socket path; assuming ${ENGINE_SOCK}" >&2
      fi
      ;;
    *)
      echo "ERROR: Unsupported CONTAINER_ENGINE '${CONTAINER_ENGINE}' (expected 'docker' or 'podman')" >&2
      exit 1
      ;;
  esac
}

# ── Flux seeding ──────────────────────────────────────────────────────────────
# Installs the Flux Operator and the FluxInstance that syncs the local OCI
# artifact, then waits for the instance to become Ready. The single kind
# cluster is the only cluster, so there is no target kubeconfig: seeding
# always runs against the current kubectl context.
#
# Requires the REGISTRY_NAME / REGISTRY_PORT globals (set by bootstrap.sh).

# KEEP IN SYNC with bootstrap.toml [charts] flux-operator: the Python
# cross-check (tests/test-bootstrap-config.py) asserts the pin exists and
# that no Git manifest declares a flux-operator HelmRelease, so the only
# drift risk is this constant.
FLUX_OPERATOR_CHART_VERSION="0.61.0"

SEED_CLEANUP_FILES=""

seed_register_cleanup() {
  SEED_CLEANUP_FILES="${SEED_CLEANUP_FILES}${1} "
}

seed_cleanup() {
  # shellcheck disable=SC2086  # intentional word splitting over registered files
  for f in $SEED_CLEANUP_FILES; do
    rm -f "$f"
  done
  SEED_CLEANUP_FILES=""
}

seed_flux() {
  local registry_name="${REGISTRY_NAME:-kaipr-registry}"
  local anon_registry_config
  anon_registry_config="$(mktemp)"
  seed_register_cleanup "$anon_registry_config"
  printf '{}\n' > "$anon_registry_config"

  # ── Install the Flux Operator ───────────────────────────────────────────────
  echo ">>> Installing Flux Operator..."
  # The empty registry config keeps helm from picking up local container
  # registry credentials when pulling the public OCI chart below.
  # --version pins the chart (bootstrap.toml [charts]) so a rerun cannot
  # resolve a different release.
  helm upgrade --install flux-operator \
    oci://ghcr.io/controlplaneio-fluxcd/charts/flux-operator \
    --version "$FLUX_OPERATOR_CHART_VERSION" \
    --namespace flux-system \
    --create-namespace \
    --wait \
    --timeout 10m \
    --registry-config "$anon_registry_config"

  # ── SOPS age decryption key secret ────────────────────────────────────────
  # Flux's kustomize-controller uses this key to decrypt *.sops.yaml manifests
  # during reconciliation (the local-ai-platform Kustomization declares
  # spec.decryption; see docs/secrets.md). Flux scans the Secret for keys
  # matching the pattern `keys.<public-key>.agekey` and passes each match to
  # the age library for decryption.
  #
  # The PRIVATE key is AGE_KEY_FILE (default age.agekey, env-overridable,
  # e.g. in .env); the matching public key is the recipient in .sops.yaml.
  # Generate the pair with:  mise run sops-keygen
  #
  # Only flux-system is seeded: this repo reconciles every Kustomization from
  # flux-system (unlike krops, which also seeds default for its
  # ResourceSet-generated Kustomizations), and spec.decryption.secretRef
  # resolves only in the Kustomization's own namespace.
  #
  # Delete first: kubectl create is not idempotent, and a re-run after an age
  # key rotation must not leave a stale key from the previous keypair behind.
  local age_key_file="${AGE_KEY_FILE:-age.agekey}"
  if [ ! -f "$age_key_file" ]; then
    echo "ERROR: age key file not found at '$age_key_file'." >&2
    echo "       Generate one with:  mise run sops-keygen" >&2
    echo "       and add its PUBLIC key to .sops.yaml. See docs/secrets.md." >&2
    exit 1
  fi
  # Validate the age key file format first (before extracting the public key)
  # so a malformed file fails loudly instead of producing an empty key name.
  local age_content missing_fields=""
  age_content="$(cat "$age_key_file")"
  printf '%s\n' "$age_content" | grep -q '^# created:' || missing_fields="${missing_fields}# created: header, "
  printf '%s\n' "$age_content" | grep -q '^# public key:' || missing_fields="${missing_fields}# public key: comment, "
  printf '%s\n' "$age_content" | grep -q '^AGE-SECRET-KEY-' || missing_fields="${missing_fields}AGE-SECRET-KEY- line, "
  if [ -n "$missing_fields" ]; then
    echo "ERROR: '$age_key_file' is not a valid age key file." >&2
    echo "       Missing: ${missing_fields%, }" >&2
    exit 1
  fi
  # Derive the public key: AGE_PUBLIC_KEY (e.g. from .env) wins, otherwise
  # age-keygen. The toolbox image does not ship the age binary, so a toolbox
  # run falls back to AGE_PUBLIC_KEY in .env when the key cannot be derived.
  local age_pubkey="${AGE_PUBLIC_KEY:-}"
  if [ -z "$age_pubkey" ]; then
    if command -v age-keygen >/dev/null 2>&1; then
      age_pubkey="$(age-keygen -y "$age_key_file")"
    else
      echo "ERROR: age-keygen not found and AGE_PUBLIC_KEY is not set." >&2
      echo "       Set AGE_PUBLIC_KEY in .env to the public key printed by" >&2
      echo "       'mise run sops-keygen'. See docs/secrets.md." >&2
      exit 1
    fi
  fi
  echo ">>> Creating sops-age decryption secret in flux-system..."
  kubectl delete secret sops-age -n flux-system --ignore-not-found
  kubectl create secret generic sops-age \
    --namespace flux-system \
    --from-file="keys.${age_pubkey}.agekey=${age_key_file}"

  # ── Install the FluxInstance via Helm ───────────────────────────────────────
  echo ">>> Installing FluxInstance via Helm..."
  # Helm 4's watcher strategy treats the FluxInstance's transient InProgress
  # condition as a terminal failure. Use the legacy chart-resource wait here,
  # then wait explicitly for the operator-owned Ready condition below.
  helm upgrade --install flux \
    oci://ghcr.io/controlplaneio-fluxcd/charts/flux-instance \
    --version "$FLUX_OPERATOR_CHART_VERSION" \
    --namespace flux-system \
    --wait=legacy \
    --timeout 10m \
    --set instance.cluster.type=kubernetes \
    --set instance.cluster.size=small \
    --set instance.cluster.multitenant=false \
    --set instance.cluster.networkPolicy=true \
    --set instance.cluster.domain=cluster.local \
    --set instance.sync.kind=OCIRepository \
    --set "instance.sync.url=oci://${registry_name}:5000/${OCI_REPOSITORY:-kaipr}" \
    --set "instance.sync.ref=${OCI_TAG:-latest}" \
    --set instance.sync.path=workload/local-host \
    --set-json 'instance.kustomize.patches=[{"patch":"- op: add\n  path: /spec/insecure\n  value: true","target":{"kind":"OCIRepository"}}]' \
    --registry-config "$anon_registry_config"

  echo ">>> Waiting for FluxInstance reconciliation to complete..."
  kubectl wait fluxinstance/flux \
    --namespace flux-system \
    --for=condition=Ready \
    --timeout=10m

  # ── Health check ────────────────────────────────────────────────────────────
  # Verify the Flux controllers are running before declaring success.
  echo ">>> Waiting for Flux controllers to be ready..."
  kubectl wait --namespace flux-system --for=condition=ready pod \
    --selector='app.kubernetes.io/part-of=flux' \
    --timeout=90s || true
}
