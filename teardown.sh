#!/usr/bin/env bash
# teardown.sh – Destroy everything bootstrap.sh created, in reverse order.
#
# Destruction order:
#   1. Remove the exported host kubeconfig
#   2. Remove the local registry container
#   3. Delete the kind cluster
#
# There is no CAPI, no pivot, and no management cluster to unwind, and no
# cloud resources to sweep: every step is idempotent, a resource that is
# already absent is skipped, and an interrupted run resumes by rerunning.
#
# Usage:
#   ./teardown.sh
#   KAIPR_PROFILE=local-host ./teardown.sh
set -euo pipefail

PROFILE="${KAIPR_PROFILE:-${1:-local-host}}"

# Names sourced from bootstrap.toml [bootstrap]; keep in sync with
# tests/test-bootstrap-config.py, which asserts them.
KIND_CLUSTER="kaipr"
REGISTRY_NAME="kaipr-registry"
KUBECONFIG_FILE="${KAIPR_KUBECONFIG:-$HOME/.kube/kaipr.yaml}"

preflight_checks() {
  case "$PROFILE" in
    local-host) ;;
    *)
      echo "ERROR: unsupported profile '$PROFILE' (expected 'local-host')" >&2
      exit 1
      ;;
  esac

  command -v kind >/dev/null 2>&1 || { echo "ERROR: kind not found in PATH" >&2; exit 1; }

  # Teardown verifies its own success, so a running engine is required:
  # without it neither the registry container nor the kind cluster can be
  # reached, and claiming completion would be a lie.
  if [ -n "${CONTAINER_ENGINE:-}" ]; then
    case "$CONTAINER_ENGINE" in
      docker|podman) ;;
      *)
        echo "ERROR: Unsupported CONTAINER_ENGINE '${CONTAINER_ENGINE}' (expected 'docker' or 'podman')" >&2
        exit 1
        ;;
    esac
    if ! command -v "$CONTAINER_ENGINE" >/dev/null 2>&1 \
      || ! "$CONTAINER_ENGINE" info >/dev/null 2>&1; then
      echo "ERROR: ${CONTAINER_ENGINE} is not running; cannot remove the registry container or the kind cluster" >&2
      exit 1
    fi
  elif command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
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
}

preflight_checks
echo ">>> Teardown of '${PROFILE}': registry '${REGISTRY_NAME}', kind cluster '${KIND_CLUSTER}'"

# ── Step 1: Remove the exported host kubeconfig ───────────────────────────────
# Independent of the cluster and safe either way, so it goes first.
if [ -f "$KUBECONFIG_FILE" ]; then
  rm -f "$KUBECONFIG_FILE"
  echo ">>> Removed kubeconfig ${KUBECONFIG_FILE}"
else
  echo ">>> Kubeconfig ${KUBECONFIG_FILE} not present; skipping"
fi

# ── Step 2: Remove the local registry container ───────────────────────────────
# It is attached to the shared 'kind' docker network, so remove it before the
# cluster goes; a dangling container on a destroyed network needs a manual
# cleanup.
if $CONTAINER_ENGINE ps -a --filter "name=^${REGISTRY_NAME}$" --format '{{.Names}}' 2>/dev/null | grep -q "^${REGISTRY_NAME}$"; then
  echo ">>> Removing registry container '${REGISTRY_NAME}'..."
  $CONTAINER_ENGINE rm -f "$REGISTRY_NAME" >/dev/null
  echo ">>> Registry container '${REGISTRY_NAME}' removed"
else
  echo ">>> Registry container '${REGISTRY_NAME}' not present; skipping"
fi

# ── Step 3: Delete the kind cluster ───────────────────────────────────────────
# Addressed by explicit name, so a stale kubectl current-context cannot
# redirect the destructive operation.
if kind get clusters 2>/dev/null | grep -q "^${KIND_CLUSTER}$"; then
  echo ">>> Deleting kind cluster '${KIND_CLUSTER}'..."
  kind delete cluster --name "$KIND_CLUSTER"
  echo ">>> Kind cluster '${KIND_CLUSTER}' deleted"
else
  echo ">>> Kind cluster '${KIND_CLUSTER}' not present; skipping"
fi

echo ""
echo ">>> Teardown complete: the bootstrap left nothing behind."
