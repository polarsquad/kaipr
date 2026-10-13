# Operations

The `local-host` environment is the only one this repository ships. Everything
below assumes it; there is no cloud account, no GitHub PAT, no GPU, and no
secrets: the single kind cluster is self-contained and its configuration is
published to a local registry from the checkout.

## Prerequisites

### Toolbox container (primary interface)

The toolbox image (`ghcr.io/polarsquad/kaipr-toolbox`) carries
`kaipr-bootstrap` plus every tool used by bootstrap and teardown. The host
needs the repository checkout and a running Docker engine or Podman 5.5+.

The toolbox image is published to `ghcr.io/polarsquad/kaipr-toolbox` for Linux
amd64 and arm64 as `X.Y.Z`, `X.Y`, and stable `latest`, signed with GitHub OIDC
and carrying a Syft SPDX JSON SBOM attestation. The `toolbox-release` workflow
publishes those tags from a matching `v*` tag. Use a published tag, or build the
current checkout only for unreleased changes:

```sh
docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
export TOOLBOX_IMAGE=kaipr-toolbox:dev
mkdir -p .kube
```

A complete raw Docker run for `local-host` is:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kind.yaml \
  "$TOOLBOX_IMAGE" local-host
```

This form assumes the standard `/var/run/docker.sock` daemon socket. Use the
wrapper below for Docker contexts or Podman installations with a different
host socket. (The positional `local-host` selects the environment; with no
subcommand the entrypoint runs the bootstrap. Use `"$TOOLBOX_IMAGE" teardown
local-host` for teardown.)

Podman socket locations differ across rootful Linux, rootless Linux, and
`podman machine`. Use the checked-in wrapper to resolve the host mount and the
socket path seen by the daemon:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh bootstrap local-host
CONTAINER_ENGINE=podman TOOLBOX_IMAGE="$TOOLBOX_IMAGE" \
  scripts/toolbox-run.sh bootstrap local-host
```

The wrapper is the lifecycle entry point. `KAIPR_PROFILE` (or the positional
profile argument after the lifecycle verb, which reaches `kaipr-bootstrap`)
selects the environment; this reference declares only `local-host`. It loads
`.env` before detecting the engine or resolving a socket for it, so a
`.env`-selected `CONTAINER_ENGINE` takes effect from the start. It resolves
`.env` exactly as mise's `env_file` does, so `scripts/toolbox-run.sh` and
`mise run bootstrap` hand the container the same values:

- `.env` wins over the process environment: a `NAME=value` prefix on the
  command line (such as `CONTAINER_ENGINE=podman` above) only applies when
  `.env` does not set `NAME`. The helper tasks follow the same rule inside
  the container (see [Helper tasks in the toolbox](#helper-tasks-in-the-toolbox)).
- Line syntax: an optional `export` prefix, whitespace trimmed around keys and
  values, `"..."` / `'...'` values taken verbatim up to the closing quote, and
  a `#` after whitespace starting a comment in an unquoted value. Lines without
  `=` or with an invalid key are skipped.

`tests/test-toolbox-run-env-precedence.py` pins this behavior and, when mise is
installed, checks it against mise itself. The wrapper then passes only a fixed
set of values into the container, built by `build_env_args` from the
`TOOLBOX_ENV_SPEC` list, split into two groups: mutable (forwarded from whatever
`.env` or the shell set) and immutable (a value the wrapper computes or
hardcodes itself; never an operator override, even from `.env`). The immutable
keys are `ENGINE_SOCK` (the daemon-side socket kind mounts) and `KUBECONFIG`
(the single file the CLI reads and rewrites). The kaipr-bootstrap CLI's own
knobs (`KAIPR_PROFILE`, `REGISTRY_PORT`, ...) are covered separately in
[bootstrap-cli.md](./bootstrap-cli.md) ("Bootstrap controls").

If an operator value is set for an immutable key, `build_env_args` prints a
warning to stderr instead of silently discarding it. The wrapper does not pass
`BOOTSTRAP_CONFIG`, `REGISTRY_READY_RETRIES`, `LOCAL_RECONCILE_TIMEOUT`, or
`KAIPR_KUBECONFIG`. Use a raw container run with explicit `-e` entries when
overriding those values.

Inside the toolbox:

- The entrypoint sets `KAIPR_TOOLBOX=1` and execs `kaipr-bootstrap`, which owns
  engine detection, the daemon-side `ENGINE_SOCK` used by kind's socket mount,
  and kind-network attach/detach.
- Bootstrap joins the `kind` network explicitly after creating (or reusing) the
  cluster; recreate and teardown detach before deleting the cluster; teardown
  re-joins first if it needs the internal API endpoint.
- Kind's internal API endpoint and `kaipr-registry:5000` then resolve by name.
- `KUBECONFIG` must name one writable file. The documented invocation uses
  `/workspace/.kube/kind.yaml`, and the CLI replaces it with kind's internal
  kubeconfig after creation.
- `/root/.kube` maps to the checkout's `.kube/`, so the exported host
  kubeconfig persists on the host as `.kube/kaipr.yaml`.

### Helper tasks in the toolbox

The one-off and helper steps (kubeconfig exports, `oci-push`) are mise tasks
defined in `mise.local-host.toml`. They run in the same toolbox image as the
lifecycle, by pointing the container's entrypoint at `mise` and running the
task from the mounted checkout. There is no wrapper verb for them.

The helper tasks are cluster tasks: they run as root, with the persisted
kubeconfig directory and the engine socket mounted like the lifecycle run:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kind.yaml \
  -e MISE_AUTO_INSTALL=0 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run <task>
```

`oci-push` additionally needs the kind network and the registry's
kind-network name, because it pushes to the registry from the network rather
than through the published host port:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kind.yaml \
  -e MISE_AUTO_INSTALL=0 --network kind \
  -e REGISTRY_HOST=kaipr-registry -e REGISTRY_PORT=5000 \
  -e KAIPR_OCI_GIT_SHA -e KAIPR_OCI_GIT_REF -e KAIPR_OCI_SOURCE_URL \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run oci-push
```

Rules that apply to every helper run:

- `--entrypoint mise` bypasses `toolbox-entrypoint.sh`, so `KAIPR_TOOLBOX` is
  not set. That is fine for these tasks: they talk to the engine through the
  mounted socket and to the registry by the `REGISTRY_HOST`/`REGISTRY_PORT`
  you pass.
- `MISE_AUTO_INSTALL=0` is required. The mounted `mise.toml` pins dev-only
  tools (`go`) that the image does not carry; without the flag mise tries to
  install them, and as a non-root user that fails with `Permission denied`
  under `/usr/local/share/mise`.
- mise loads `/workspace/.env` (`env_file` in `mise.toml`) and its values
  override the process environment.
- Podman: replace `docker` with `podman` and the socket source with the one
  `scripts/toolbox-run.sh` resolves (`podman info --format
  '{{.Host.RemoteSocket.Path}}'`).

Host-side on purpose: `mise run validate` (repository development, see
[Validation](#validation)) and `mise -E local-host run inference-demo` (the
browser is on the host; see [Verifying the AI platform](#verifying-the-ai-platform)).

### Verifying a toolbox release

After a release is published, replace `X.Y.Z` in these commands with the
matching Cargo and Git tag version:

```sh
IMAGE=ghcr.io/polarsquad/kaipr-toolbox:X.Y.Z
IDENTITY=https://github.com/polarsquad/kaipr/.github/workflows/toolbox-release.yml@refs/tags/vX.Y.Z

cosign verify \
  --certificate-identity "$IDENTITY" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$IMAGE"

cosign verify-attestation \
  --type spdxjson \
  --certificate-identity "$IDENTITY" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$IMAGE"
```

## Configuration

Copy the env template and fill it in. Both the lifecycle wrapper
(`scripts/toolbox-run.sh`) and the helper mise tasks (through mise's `env_file`,
inside the toolbox) load `.env` automatically, and it is gitignored:

```sh
cp .env.example .env
$EDITOR .env
```

For `local-host` the only values that matter are the optional container-engine
override (`CONTAINER_ENGINE`) and the optional registry port
(`REGISTRY_PORT`); there are no cloud credentials to set and no secrets to
manage.

Repository-owned lifecycle configuration lives in `bootstrap.toml`. It defines
the environment names, the cluster/registry names, the imperative chart
versions, and each environment's sync source and path. `kaipr-bootstrap` reads
it from the working directory unless `BOOTSTRAP_CONFIG` selects another path.
Runtime environment variables take precedence over configurable defaults, and
`mise run validate` cross-checks the file against the manifests.

## Bootstrap

The local-host lifecycle boots through the wrapper:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh bootstrap local-host
```

This initial imperative phase performs these steps:

1. Creates the `kaipr` kind cluster (or reuses it; `--recreate` replaces it).
2. Starts a local Docker Registry container (`registry:2`) on the kind network
   (accessible at `localhost:5001` by default, or `kaipr-registry:5000` on the
   kind network). It is idempotent: it restarts if stopped and is a no-op if
   already running.
3. Publishes the `workload/local-host/` folder as the initial `kaipr:latest`
   OCI artifact (the path is preserved inside the artifact).
4. Installs the Flux Operator (Helm, pinned in `bootstrap.toml`).
5. Installs a `FluxInstance` in `flux-system` that syncs
   `workload/local-host/` from that artifact and hands off to GitOps.
6. Waits for reconciliation: the Flux controller, then the root Kustomization
   `local-ai-platform` and its CRD-layer parent.
7. Exports the host kubeconfig to `.kube/kaipr.yaml` (in the toolbox; a native
   run writes `~/.kube/kaipr.yaml`) via `kind get kubeconfig`. kind publishes
   the API port on localhost, so the file works from the host with no
   endpoint rewrite.

Flux then reconciles `workload/local-host/` from the artifact, bringing up the
`ai-platform` tree. kind is intended for local development and testing, not
production.

Together, these stages make `local-host` an end-to-end environment: one command
bootstraps a local cluster, publishes and reconciles the OCI configuration, and
reconciles a reachable AI platform.

**OCI Registry (local-host environment):**
- Provides a local container registry for development workflows
- Enables developers to build and push OCI artifacts from git checkouts
- Flux syncs and deploys the OCI artifact without external dependencies
- Configurable via `REGISTRY_PORT` env var (defaults to 5001)
- Idempotent: restarts if stopped, no action needed if already running

`scripts/toolbox-run.sh` resolves the current SHA, branch (or `detached`), and
origin URL on the host after loading `.env`, then forwards them as
`KAIPR_OCI_GIT_SHA`, `KAIPR_OCI_GIT_REF`, and `KAIPR_OCI_SOURCE_URL`. These
names are reserved for checkout metadata; do not set them in `.env`, which mise
loads again inside the container. The raw helper below supplies all three
explicitly. OCI publication uses mise's configuration root and never follows a
linked worktree's host-only `.git` pointer when metadata is supplied. Native
runs can derive metadata from Git; a partial or empty supplied tuple fails with
an error. Without an origin, the source is a `file://` URL for the checkout.

**Republish after a change:**
```bash
# Republish the local workload folder after making changes.
# Resolve metadata on the host, including when this checkout is a linked worktree.
export KAIPR_OCI_GIT_SHA="$(git rev-parse HEAD)"
export KAIPR_OCI_GIT_REF="$(git branch --show-current)"
export KAIPR_OCI_GIT_REF="${KAIPR_OCI_GIT_REF:-detached}"
export KAIPR_OCI_SOURCE_URL="$(git config --get remote.origin.url || true)"
export KAIPR_OCI_SOURCE_URL="${KAIPR_OCI_SOURCE_URL:-file://$PWD}"
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kind.yaml \
  -e MISE_AUTO_INSTALL=0 --network kind -e REGISTRY_HOST=kaipr-registry -e REGISTRY_PORT=5000 \
  -e KAIPR_OCI_GIT_SHA -e KAIPR_OCI_GIT_REF -e KAIPR_OCI_SOURCE_URL \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run oci-push

# Optional overrides: add -e OCI_REPOSITORY=my-config -e OCI_TAG=v1 to the same run.

# The FluxInstance pulls and reconciles the artifact's workload/local-host
# kustomizations. The bootstrap configures kind's containerd to mirror
# localhost:5001 to the registry's in-cluster endpoint, kaipr-registry:5000,
# which is the endpoint the toolbox run above pushes to over the kind network.
```

The artifact contains only `workload/local-host/`, preserving that directory
path when the artifact is pulled. Keeping the source scope narrow keeps the
artifact small and avoids packaging anything else from the checkout.

Watch reconciliation after a toolbox run with the persisted kubeconfig:

```sh
export KUBECONFIG="$PWD/.kube/kaipr.yaml"
flux get kustomizations --watch
```

### Exporting the kubeconfig

The bootstrap exports the host kubeconfig itself. To re-export it after
teardown-plus-bootstrap (the published port changes when the cluster container
is recreated), run the `kubeconfigs` helper task, which writes
`.kube/kaipr.yaml` from `kind get kubeconfig --name kaipr`:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e MISE_AUTO_INSTALL=0 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run kubeconfigs
```

Because kind publishes the API port on localhost, the exported file works from
the host and from the toolbox with no endpoint rewrite on any OS.

### Verifying the AI platform

The cluster reconciles `workload/local-host/ai-platform/` from the same OCI
artifact: the CRD layer (`crd/`) first, then the app layer (`app/`) once the
CRDs are Ready. Verify the stages of the request path:

```sh
export KUBECONFIG="$PWD/.kube/kaipr.yaml"

# model servers (2 replicas, labeled app=vllm-sim)
kubectl get pods -n ai-platform -l app=vllm-sim

# the pool selects them and references the EPP
kubectl get inferencepool vllm-sim -n ai-platform
kubectl get deployment vllm-sim-epp -n ai-platform

# the LLM route is PROGRAMMED and ATTACHED to the gateway
kubectl get httproute vllm-sim-llm -n ai-platform -o jsonpath='{.status.conditions}'

# the agentgateway LLM policies are bound
kubectl get agentgatewaybackend vllm-sim -n ai-platform
kubectl get agentgatewaypolicy vllm-sim-token-budget -n ai-platform
```

Then send a chat request through the gateway:

```sh
kubectl port-forward -n ai-platform svc/inference-gateway 18080:80
curl -s http://localhost:18080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"Qwen/Qwen3-32B","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'
```

For a browser interface instead of curl, run the demo host-side (the port
forward is managed by the task):

```sh
mise -E local-host run inference-demo
```

The full component list, the pinned versions, the CRD prerequisites, and the
vLLM overlay for a real model are in [Inference platform](./inference.md).

The cluster runs the kind 0.33.0 default node image, Kubernetes v1.37.0. kind's
default Kindnet CNI provides networking. The cluster needs access to the
container-engine socket, which the bootstrap mounts automatically.

Local-host bootstrap streams the Flux controller error logs while it waits for
the root Kustomizations, and returns once they are Ready. Filtering the stream
to errors avoids showing normal startup retries and advisory messages as
apparent failures. Each readiness wait defaults to 15 minutes and can be
changed with `LOCAL_RECONCILE_TIMEOUT` in a raw container or fallback native
run. The current wrapper does not forward that override.

### Verifying the full chain

For the local-host end-to-end chain:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh bootstrap local-host
export KUBECONFIG="$PWD/.kube/kaipr.yaml"
flux get all --all-namespaces
kubectl get all -n ai-platform
# then the AI-platform verification above
```

Bootstrap does not return until the root Kustomization is Ready. The final
port-forward verifies that the Flux instance successfully delivered the
application.

## Teardown

The wrapper runs the Rust subcommand in the toolbox:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh teardown local-host
```

The retained `./teardown.sh` reference path and a native
`kaipr-bootstrap teardown [PROFILE]` run are the fallbacks. Teardown reads the
resource names from `bootstrap.toml` and removes everything the bootstrap
created, in reverse creation order:

1. The exported host kubeconfig (`.kube/kaipr.yaml` in the toolbox,
   `~/.kube/kaipr.yaml` for a native run).
2. The local registry container (`kaipr-registry`).
3. The kind cluster (`kaipr`) and its containers.

Each step is idempotent: a resource that is already absent is skipped, so an
interrupted run can be resumed by rerunning. Teardown needs a running container
engine (docker or podman) to remove the registry and the cluster; a
required-tool failure happens before mutation. There is no CAPI inventory, no
management cluster, and no cloud resources to unwind.

## Validation

Run the repository validation before pushing:

```sh
mise run validate
```

The task checks shell syntax for the retained lifecycle scripts, runs the
`bootstrap.toml` manifest cross-check and the toolbox/OCI/documentation tests,
and builds every kustomize overlay under `workload/`. It is repository
development, so it stays a host task on purpose (it needs the pinned Python,
`uv`, and `kubectl`). Contributors without a host toolchain can run the same
task in the toolbox, redirecting the uv environment off the mount:

```sh
docker run --rm -v "$PWD:/workspace" -w /workspace \
  -e MISE_AUTO_INSTALL=0 -e UV_PROJECT_ENVIRONMENT=/tmp/kaipr-venv \
  --entrypoint mise "$TOOLBOX_IMAGE" run validate
```

`.github/workflows/validate.yml` separately builds every overlay, cross-checks
`bootstrap.toml`, and lints YAML on pushes to `main` and on pull requests.
`.github/workflows/bootstrap-rs.yml` runs Rust format, clippy, build, and tests,
then builds and smokes the toolbox image when its inputs change.
