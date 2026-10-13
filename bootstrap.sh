#!/usr/bin/env bash
# bootstrap.sh – One-time imperative bootstrap for the local kind cluster.
# Everything after this script runs is driven by GitOps (Flux).
#
# This is the native (host) path; the toolbox path is
# `scripts/toolbox-run.sh bootstrap` (the Rust engine, bootstrap-rs). The two
# implement the same flow, so the steps below mirror `kaipr-bootstrap`'s
# run_bootstrap: ensure the kind cluster, bootstrap the local registry and
# publish the OCI artifact, seed Flux (operator + FluxInstance), watch
# reconciliation, and export the kubeconfig.
#
# Usage:
#   ./bootstrap.sh                    # local-host, reuse an existing cluster
#   ./bootstrap.sh --recreate         # delete and rebuild the cluster first
#   KAIPR_PROFILE=local-host ./bootstrap.sh
set -euo pipefail

source "$(dirname "$0")/bootstrap-common.sh"

# ── Arguments ─────────────────────────────────────────────────────────────────
RECREATE=0
POSITIONAL=()
for arg in "$@"; do
  case "$arg" in
    --recreate) RECREATE=1 ;;
    --) ;;
    -h|--help)
      echo "Usage: ./bootstrap.sh [--recreate] [profile]" >&2
      exit 0
      ;;
    -*)
      echo "ERROR: unknown option '$arg' (expected --recreate)" >&2
      exit 2
      ;;
    *) POSITIONAL+=("$arg") ;;
  esac
done

# Names sourced from bootstrap.toml [bootstrap]; keep in sync with
# tests/test-bootstrap-config.py, which asserts them.
KIND_CLUSTER="kaipr"
KIND_CONTEXT="kind-kaipr"
REGISTRY_NAME="kaipr-registry"
FLUX_NAMESPACE="flux-system"
PROFILE="${KAIPR_PROFILE:-${POSITIONAL[0]:-local-host}}"
REGISTRY_PORT="${REGISTRY_PORT:-5001}"
REGISTRY_READY_RETRIES="${REGISTRY_READY_RETRIES:-120}"
LOCAL_RECONCILE_TIMEOUT="${LOCAL_RECONCILE_TIMEOUT:-15m}"
KUBECONFIG_FILE="${KAIPR_KUBECONFIG:-$HOME/.kube/kaipr.yaml}"

preflight_checks() {
  case "$PROFILE" in
    local-host) ;;
    *)
      echo "ERROR: unsupported profile '$PROFILE' (expected 'local-host')" >&2
      exit 1
      ;;
  esac

  for cmd in curl kind helm kubectl mise; do
    command -v "$cmd" >/dev/null 2>&1 || { echo "ERROR: $cmd not found in PATH"; exit 1; }
  done

  detect_container_engine
}

preflight_checks
echo ">>> Using container engine: ${CONTAINER_ENGINE} (socket: ${ENGINE_SOCK})"

# ── Step 1: Ensure the kind cluster ───────────────────────────────────────────
# Reuse by default; --recreate (or no existing cluster) (re)builds it. The
# node mounts the host engine socket at the standard Docker path so in-cluster
# components reach a Docker-compatible API whether the backend is Docker or
# Podman. The containerd patch points the cluster at the local registry.
if kind get clusters 2>/dev/null | grep -q "^${KIND_CLUSTER}$"; then
  if [ "$RECREATE" = 1 ]; then
    echo ">>> Cluster '${KIND_CLUSTER}' exists – recreating (--recreate)..."
    kind delete cluster --name "$KIND_CLUSTER"
  else
    echo ">>> Reusing existing kind cluster '${KIND_CLUSTER}'"
    kubectl config use-context "$KIND_CONTEXT"
    echo ">>> Waiting for cluster node to be ready..."
    kubectl wait --for=condition=Ready node --all --timeout=120s
  fi
else
  echo ">>> Creating kind cluster '${KIND_CLUSTER}'..."
  kind create cluster --name "$KIND_CLUSTER" --config - <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
containerdConfigPatches:
  - |-
    [plugins."io.containerd.grpc.v1.cri".registry.mirrors."localhost:${REGISTRY_PORT}"]
      endpoint = ["http://${REGISTRY_NAME}:5000"]
nodes:
  - role: control-plane
    extraMounts:
      - hostPath: ${ENGINE_SOCK}
        containerPath: /var/run/docker.sock
EOF
  echo ">>> Waiting for cluster node to be ready..."
  kubectl config use-context "$KIND_CONTEXT"
  kubectl wait --for=condition=Ready node --all --timeout=120s
fi

# ── Step 2: Bootstrap the local container registry ────────────────────────────
echo ">>> Bootstrapping local container registry..."
registry_exists() {
  $CONTAINER_ENGINE ps -a --filter "name=^${REGISTRY_NAME}$" --format '{{.Names}}' 2>/dev/null | grep -q "^${REGISTRY_NAME}$"
}
registry_running() {
  $CONTAINER_ENGINE ps --filter "name=^${REGISTRY_NAME}$" --format '{{.Names}}' 2>/dev/null | grep -q "^${REGISTRY_NAME}$"
}
create_registry() {
  $CONTAINER_ENGINE run -d \
    --name "$REGISTRY_NAME" \
    --network kind \
    -p "127.0.0.1:${REGISTRY_PORT}:5000" \
    registry:2 >/dev/null
}

if ! registry_exists; then
  echo "    Creating registry container '${REGISTRY_NAME}'..."
  create_registry
  echo "    Registry created and running: localhost:${REGISTRY_PORT}"
else
  # A name match alone proves nothing about configuration. Verify the host
  # port binding and the kind-network attachment; a stale container bound
  # elsewhere or detached from the network answers the host probe and then
  # fails in-cluster.
  published_port="$($CONTAINER_ENGINE port "$REGISTRY_NAME" 5000/tcp 2>/dev/null | head -1 | cut -d: -f2 || true)"
  in_kind_network="$($CONTAINER_ENGINE inspect "$REGISTRY_NAME" --format '{{json .NetworkSettings.Networks}}' 2>/dev/null || true)"
  if [ "$published_port" != "$REGISTRY_PORT" ] || ! printf '%s' "$in_kind_network" | grep -q '"kind"'; then
    echo "    Existing registry misconfigured (port or network); recreating..."
    $CONTAINER_ENGINE rm -f "$REGISTRY_NAME" >/dev/null
    create_registry
    echo "    Registry recreated: localhost:${REGISTRY_PORT}"
  elif ! registry_running; then
    echo "    Restarting stopped registry..."
    $CONTAINER_ENGINE start "$REGISTRY_NAME" >/dev/null
    echo "    Registry restarted: localhost:${REGISTRY_PORT}"
  else
    echo "    Registry already running: localhost:${REGISTRY_PORT}"
  fi
fi

echo ">>> Waiting for local registry API at localhost:${REGISTRY_PORT}..."
if ! curl --fail --silent --show-error \
  --retry "$REGISTRY_READY_RETRIES" \
  --retry-connrefused \
  --retry-delay 1 \
  "http://localhost:${REGISTRY_PORT}/v2/" >/dev/null; then
  echo "ERROR: local registry did not become ready at localhost:${REGISTRY_PORT}" >&2
  exit 1
fi

echo ">>> Publishing initial OCI artifact from the local Git checkout..."
# Forward the resolved values: the mise oci-push task reads them from its own
# environment. A host run always reaches the registry on localhost.
REGISTRY_HOST="localhost" \
REGISTRY_PORT="$REGISTRY_PORT" \
OCI_REPOSITORY="${OCI_REPOSITORY:-kaipr}" \
OCI_TAG="${OCI_TAG:-latest}" \
mise -E "$PROFILE" run oci-push
echo ">>> Initial OCI artifact is available at oci://localhost:${REGISTRY_PORT}/${OCI_REPOSITORY:-kaipr}:${OCI_TAG:-latest}"

# ── Steps 3–4: Seed Flux (operator, then FluxInstance) ────────────────────────
cleanup_bootstrap() {
  seed_cleanup
}
trap cleanup_bootstrap EXIT
seed_flux

# ── Step 5: Watch local-host reconciliation ──────────────────────────────────
# The root Kustomization appears once the operator has reconciled the
# FluxInstance; wait for it before asking kubectl to wait for readiness
# (kubectl wait errors on a not-yet-created resource).
echo ""
echo ">>> Flux reconciliation progress"
reconcile_discovery_attempts=0
until kubectl get kustomization --all --namespace "$FLUX_NAMESPACE" >/dev/null 2>&1; do
  reconcile_discovery_attempts=$((reconcile_discovery_attempts + 1))
  if [ "$reconcile_discovery_attempts" -ge 60 ]; then
    echo "ERROR: no Flux Kustomization was created within 2 minutes" >&2
    flux get kustomizations
    exit 1
  fi
  sleep 2
done

# Stream Flux errors in the bootstrap terminal while we wait.
flux logs --all-namespaces --follow --level=error --since=10m &
FLUX_LOG_PID=$!
cleanup_watch() {
  kill "$FLUX_LOG_PID" >/dev/null 2>&1 || true
  wait "$FLUX_LOG_PID" >/dev/null 2>&1 || true
}
trap cleanup_watch EXIT

echo ">>> Waiting for the AI platform to be ready..."
if ! kubectl wait kustomization --all \
    --namespace "$FLUX_NAMESPACE" \
    --for=condition=Ready \
    --timeout="$LOCAL_RECONCILE_TIMEOUT"; then
  echo "ERROR: reconciliation did not complete within ${LOCAL_RECONCILE_TIMEOUT}" >&2
  flux get kustomizations
  exit 1
fi
trap cleanup_bootstrap EXIT

# ── Step 6: Export the host kubeconfig ────────────────────────────────────────
# kind publishes the API port on localhost, so the exported kubeconfig works
# from the host with no endpoint rewrite.
echo ""
mkdir -p "$(dirname "$KUBECONFIG_FILE")"
kind get kubeconfig --name "$KIND_CLUSTER" > "$KUBECONFIG_FILE"
chmod 600 "$KUBECONFIG_FILE"
echo ">>> Kubeconfig written to ${KUBECONFIG_FILE}"

# ── Done ──────────────────────────────────────────────────────────────────────
# Everything else is driven by GitOps. The FluxInstance above syncs the local
# OCI artifact (path workload/local-host), whose top-level kustomization.yaml
# wires in the AI platform Kustomizations with the correct dependsOn ordering.
echo ""
echo ">>> Local-host profile complete: Flux is reconciling from the local OCI artifact"
echo ">>> Local registry: localhost:${REGISTRY_PORT} (cluster endpoint: ${REGISTRY_NAME}:5000)"
echo ">>> OCI source: oci://${REGISTRY_NAME}:5000/${OCI_REPOSITORY:-kaipr}:${OCI_TAG:-latest} (path: workload/local-host)"
echo ">>> Kubeconfig: ${KUBECONFIG_FILE}"
echo ">>> Watch progress with: KUBECONFIG=${KUBECONFIG_FILE} flux get kustomizations --watch"
