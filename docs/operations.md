# Operations

The `local-host` environment is the only one this repository ships. Everything
below assumes it; there is no cloud account, no GitHub PAT, and no GPU.

## Prerequisites

### Toolbox container (primary interface)

The toolbox image (`ghcr.io/polarsquad/kaipr-toolbox`) carries
`kaipr-bootstrap` plus every tool used by bootstrap, pivot, and teardown. The
host needs the repository checkout and a running Docker engine or Podman 5.5+.

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
host socket.

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
hardcodes itself; never an operator override, even from `.env`). The
kaipr-bootstrap CLI's own knobs (`KAIPR_PROFILE`, `REGISTRY_PORT`, ...) are
covered separately in [bootstrap-cli.md](./bootstrap-cli.md) ("Bootstrap and
pivot controls").

If an operator value is set for an immutable key, `build_env_args` prints a
warning to stderr instead of silently discarding it. It does not pass
`BOOTSTRAP_CONFIG`, `REGISTRY_READY_RETRIES`, `LOCAL_RECONCILE_TIMEOUT`,
`MGMT_KUBECONFIG`, `MGMT_READY_TIMEOUT`, `MGMT_POLL_INTERVAL`,
`BOOTSTRAP_KUBECONTEXT`, or the teardown controls `FORCE_KIND_DELETE`,
`CLUSTER_DELETE_TIMEOUT`, and `PROVIDER_DELETE_TIMEOUT`. Use a raw container run
with explicit `-e` entries when overriding those values.

Inside the toolbox:

- The entrypoint sets `KAIPR_TOOLBOX=1` and execs `kaipr-bootstrap`, which owns
  engine detection, the daemon-side `ENGINE_SOCK` used by kind's socket mount,
  and kind-network attach/detach.
- Bootstrap joins the `kind` network explicitly after creating (or reusing) the
  cluster; recreate, pivot, and teardown detach before deleting the bootstrap
  cluster; teardown re-joins first if it needs the internal API endpoint.
- Kind's internal API endpoint and `kaipr-registry:5000` then resolve by name.
- Host-only CAPD endpoint rewrites are skipped because the recorded endpoints
  already resolve on that network.
- On macOS the persisted `.kube/kaipr-mgmt.yaml` keeps the `kind` network
  address, which Docker Desktop does not route; see
  [Host-side access after a toolbox local-host run (macOS)](#host-side-access-after-a-toolbox-local-host-run-macos)
  for the host rewrite.
- `KUBECONFIG` must name one writable file. The documented invocation uses
  `/workspace/.kube/kind.yaml`, and the CLI replaces it with kind's internal
  kubeconfig after creation.
- `/root/.kube` maps to the checkout's `.kube/`, so the exported management
  kubeconfig persists on the host as `.kube/kaipr-mgmt.yaml`.

### Helper tasks in the toolbox

The one-off and helper steps (SOPS key work, kubeconfig exports, `oci-push`)
are mise tasks defined in `mise.toml` and `mise.local-host.toml`. They run in
the same toolbox image as the lifecycle, by pointing the container's entrypoint
at `mise` and running the task from the mounted checkout. There is no wrapper
verb for them; they all follow one of two shapes.

Repo-only tasks (`sops-*`) run as your own user so the files they write are
owned by you:

```sh
export TOOLBOX_IMAGE=ghcr.io/polarsquad/kaipr-toolbox:latest   # or kaipr-toolbox:dev
docker run --rm -it --user "$(id -u):$(id -g)" -e HOME=/tmp \
  -v "$PWD:/workspace" -w /workspace \
  -e MISE_AUTO_INSTALL=0 \
  --entrypoint mise "$TOOLBOX_IMAGE" run <task> [args]
```

Cluster tasks (`mgmt-kubeconfig`, `kubeconfigs`, `oci-push`) run as root, with
the persisted kubeconfig directory and the engine socket mounted like the
lifecycle run:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kaipr-mgmt.yaml \
  -e MISE_AUTO_INSTALL=0 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run <task>
```

Rules that apply to every helper run:

- `--entrypoint mise` bypasses `toolbox-entrypoint.sh`, so `KAIPR_TOOLBOX` is
  not set. The local-host kubeconfig tasks need it (`-e KAIPR_TOOLBOX=1`)
  together with `--network kind`: they then keep the CAPD-recorded
  kind-network endpoint instead of rewriting it to `127.0.0.1`, which inside a
  container is the container itself. A kubeconfig exported that way works from
  later toolbox runs on the kind network, not from host `kubectl`.
- `MISE_AUTO_INSTALL=0` is required. The mounted `mise.toml` pins dev-only tools
  (`zarf`, `go`) that the image does not carry; without the flag mise tries to
  install them, and as a non-root user that fails with `Permission denied`
  under `/usr/local/share/mise`.
- mise loads `/workspace/.env` (`env_file` in `mise.toml`) and its values
  override the process environment.
- Podman: replace `docker` with `podman` and the socket source with the one
  `scripts/toolbox-run.sh` resolves (`podman info --format
  '{{.Host.RemoteSocket.Path}}'`).

Host-side on purpose: `mise run validate` (repository development, see
[Validation](#validation)) and `mise -E local-host run podinfo-port-forward`
and `mise -E local-host run inference-demo` (the browser is on the host; a
toolbox form is shown with the local-host chain below).

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
(`REGISTRY_PORT`); there are no cloud credentials to set.

Repository-owned lifecycle configuration lives in `bootstrap.toml`. It defines
the environment names, sync paths, management clusters, imperative chart
versions, provider manifests, and teardown targets. `kaipr-bootstrap` reads it
from the working directory unless `BOOTSTRAP_CONFIG` selects another path.
Runtime environment variables take precedence over configurable defaults, and
`mise run validate` cross-checks the file against the manifests.

## Bootstrap

The local-host lifecycle boots through the wrapper:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh bootstrap local-host
```

This initial imperative phase performs these steps:

1. Creates the `mgmt` kind cluster.
2. Starts a local Docker Registry container (`registry:2`) on the host
   (accessible at `localhost:5001` by default, or `kaipr-registry:5000` on the
   kind network). It is idempotent: it restarts if stopped and is a no-op if
   already running.
3. Installs the Flux Operator (Helm).
4. Publishes the `mgmt/local-host/` and `workload/local-host/` folders as the
   initial `kaipr:latest` OCI artifact.
5. Installs a `FluxInstance` that syncs `mgmt/local-host/` from that artifact
   and hands off to GitOps.
6. Pivots: moves the CAPI inventory into the self-managed management cluster
   and deletes the kind cluster (see [Pivot recovery](#pivot-recovery)).

Flux then installs the CAPI core, kubeadm, and Docker infrastructure providers
and creates `local-workload`, a one-control-plane/one-worker Kubernetes cluster
in containers. The management cluster then installs a Flux Operator and
FluxInstance on `local-workload`; that instance reconciles
`workload/local-host/` from the same OCI artifact, bringing up both the
`podinfo` demo app and the `ai-platform` tree. CAPD is intended for local
development and testing, not production.

Together, these stages make `local-host` an end-to-end environment: one command
bootstraps the management control plane, publishes and reconciles the OCI
configuration, provisions a workload cluster through CAPI, installs a distinct
Flux control plane on that cluster, and reconciles a reachable workload. It
exercises the complete cluster-to-workload GitOps lifecycle locally.

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
# Republish the local management and workload folders after making changes.
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
  -e KUBECONFIG=/workspace/.kube/kaipr-mgmt.yaml \
  -e MISE_AUTO_INSTALL=0 --network kind -e REGISTRY_HOST=kaipr-registry -e REGISTRY_PORT=5000 \
  -e KAIPR_OCI_GIT_SHA -e KAIPR_OCI_GIT_REF -e KAIPR_OCI_SOURCE_URL \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run oci-push

# Optional overrides: add -e OCI_REPOSITORY=my-config -e OCI_TAG=v1 to the same run.

# FluxInstance pulls and reconciles the artifact's mgmt/local-host kustomization.
# The bootstrap configures kind's containerd to mirror localhost:5001 to the
# registry's in-cluster endpoint, kaipr-registry:5000, which is the endpoint
# the toolbox run above pushes to over the kind network.
```

The artifact contains only `mgmt/local-host/` and `workload/local-host/`,
preserving those directory paths when the artifact is pulled. Keeping the
source scope narrow also prevents local credentials and age private keys
elsewhere in the repository from being packaged.

Watch reconciliation after a toolbox run with the persisted management
kubeconfig:

```sh
export KUBECONFIG="$PWD/.kube/kaipr-mgmt.yaml"
flux get kustomizations --watch
```

After a toolbox `local-host` run on macOS the persisted file carries the
`kind` Docker network address, which Docker Desktop does not route; export the
host copy from
[Host-side access after a toolbox local-host run (macOS)](#host-side-access-after-a-toolbox-local-host-run-macos)
instead.

### Exporting the workload kubeconfig

Export the CAPD workload kubeconfig after the `local-workload` cluster reports
Ready. The toolbox run keeps the kind-network endpoint, so the file is read back
through the toolbox too:

```sh
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kaipr-mgmt.yaml \
  -e MISE_AUTO_INSTALL=0 --network kind -e KAIPR_TOOLBOX=1 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run kubeconfigs
docker run --rm --network kind -v "$PWD:/workspace" -w /workspace \
  --entrypoint kubectl "$TOOLBOX_IMAGE" --kubeconfig local-workload.kubeconfig get nodes
```

The host task form of `kubeconfigs` still reads the persisted management
kubeconfig before it can export the workload one, so on macOS it must run
against the host copy from
[Host-side access after a toolbox local-host run (macOS)](#host-side-access-after-a-toolbox-local-host-run-macos).

### Verifying the AI platform

The workload cluster reconciles `workload/local-host/ai-platform/` from the
same OCI artifact as `podinfo`. Verify the stages of the request path:

```sh
export KUBECONFIG="$PWD/.kube/local-workload.kubeconfig"

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

The full component list, the pinned versions, the CRD prerequisites, and the
vLLM overlay for a real model are in [Inference platform](./inference.md).

### Viewing podinfo

The local workload Flux instance installs Podinfo from its OCI Helm chart.
Open it in a host browser by running the port-forward in a separate terminal.
This is the one helper that stays a host task on purpose (the browser is on the
host); it needs a host `kubectl` and a kubeconfig with the `127.0.0.1` endpoint,
which `mise -E local-host run kubeconfigs` on the host produces:

```sh
mise -E local-host run podinfo-port-forward
```

On an engine-only host, publish the port from a toolbox run on the kind network
instead:

```sh
docker run --rm -it --network kind -p 9898:9898 \
  -v "$PWD:/workspace" -w /workspace -e MISE_AUTO_INSTALL=0 \
  --entrypoint kubectl "$TOOLBOX_IMAGE" --kubeconfig local-workload.kubeconfig \
  port-forward --namespace podinfo --address 0.0.0.0 service/podinfo 9898:9898
```

The workload uses Kubernetes v1.36.4. A CAPI ClusterResourceSet installs a
pinned Kindnet daemon as its CNI before the Flux addons are delivered. The
management cluster needs access to the container-engine socket, which the
bootstrap mounts automatically.

Local-host bootstrap first waits for the management `flux-apps` Kustomization
without printing transient `Unknown` status rows. After `flux-apps` becomes
Ready, it connects to `local-workload`, streams the workload Flux controller
error logs, and returns after the workload root Kustomization becomes Ready.
Filtering the workload stream to errors avoids showing normal startup retries
and advisory messages as apparent failures. Each readiness wait defaults to 15
minutes and can be changed with `LOCAL_RECONCILE_TIMEOUT` in a raw container or
fallback native run. The current wrapper does not forward that override.

### Host-side access after a toolbox local-host run (macOS)

The pivot exports the management kubeconfig with the API server address CAPD
recorded: the `local-management-lb` container IP on the `kind` Docker network.
Native local-host runs rewrite that address to the localhost port, but toolbox
runs skip the rewrite (`should_rewrite_capd_endpoint` in
`bootstrap-rs/src/main.rs`) because the address resolves on the `kind` network
the toolbox joins. Docker Desktop on macOS runs the engine in a VM and does not
route the `kind` network from the host, so host commands against the persisted
`.kube/kaipr-mgmt.yaml` time out. Rewrite a host copy to the port kind publishes
on localhost:

```sh
cp .kube/kaipr-mgmt.yaml .kube/kaipr-mgmt.host.yaml
PORT=$(docker port local-management-lb 6443/tcp | head -1 | sed 's/.*://')
kubectl config set-cluster local-management --server="https://127.0.0.1:${PORT}" \
  --kubeconfig .kube/kaipr-mgmt.host.yaml
```

Use the host copy for every host command that talks to the management cluster:

```sh
export KUBECONFIG="$PWD/.kube/kaipr-mgmt.host.yaml"
flux get kustomizations --watch
```

The host form of the `kubeconfigs` task reads the management kubeconfig before
it can export the workload one, so run it with the host copy exported; without
`KAIPR_TOOLBOX` it rewrites the workload endpoint to `127.0.0.1`, and
`podinfo-port-forward` then needs no further changes:

```sh
mise -E local-host run podinfo-port-forward
```

The published port belongs to the `local-management-lb` container and stays the
same while that container exists. It changes when the container is recreated,
for example after teardown plus bootstrap. Re-run the `docker port` and
`kubectl config set-cluster` lines whenever `docker port local-management-lb
6443/tcp` reports a different port than the kubeconfig carries.

With Podman on macOS the same limitation applies inside the podman machine VM;
use `podman port` in place of `docker port`.

On Linux the host routes to the `kind` network directly, so
`.kube/kaipr-mgmt.yaml` works as-is and no rewrite is needed.

The unaffected alternative is a one-off toolbox container attached to the
`kind` network, where the recorded address resolves with no rewrite:

```sh
docker run --rm -it --network kind \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/root/.kube/kaipr-mgmt.yaml \
  --entrypoint flux \
  "$TOOLBOX_IMAGE" get kustomizations --watch
```

Use `--entrypoint kubectl` for kubectl commands. The `mise` helper tasks are
host forms; run them against the rewritten copy.

Teardown is unaffected: it reads the management kubeconfig from inside the
toolbox, which is attached to the `kind` network.

### Verifying the full chain

For the local-host end-to-end chain:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh bootstrap local-host
docker run --rm -it \
  -v "$PWD:/workspace" -w /workspace \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$PWD/.kube:/root/.kube" \
  -e KUBECONFIG=/workspace/.kube/kaipr-mgmt.yaml \
  -e MISE_AUTO_INSTALL=0 --network kind -e KAIPR_TOOLBOX=1 \
  --entrypoint mise "$TOOLBOX_IMAGE" -E local-host run kubeconfigs
docker run --rm --network kind -v "$PWD:/workspace" -w /workspace \
  --entrypoint flux "$TOOLBOX_IMAGE" --kubeconfig local-workload.kubeconfig get all --all-namespaces
# then the podinfo and AI-platform verification above, and browse to http://localhost:9898
```

The toolbox form above needs no rewrite: the runs stay on the `kind` network.
Running those steps as host commands instead needs the host copy from
[Host-side access after a toolbox local-host run (macOS)](#host-side-access-after-a-toolbox-local-host-run-macos)
in place of `.kube/kaipr-mgmt.yaml`.

Bootstrap does not return until the management and workload root
Kustomizations are Ready. The final port-forward verifies that the workload
Flux instance successfully delivered the application.

## Pivot recovery

Bootstrap ends with a pivot: the CAPI inventory moves from the local `mgmt`
kind cluster into the self-managed management cluster, and the kind cluster is
deleted. `scripts/toolbox-run.sh bootstrap` runs the pivot by default
(`BOOTSTRAP_PIVOT=0` opts out). `scripts/toolbox-run.sh pivot` starts the same
rerun-safe CLI and resumes through the pivot; there is no separate pivot
subcommand. See [the bootstrap CLI](./bootstrap-cli.md) for its interface and
controls.

`clusterctl move` is re-runnable: an object is deleted from the source kind
cluster only after it was created on the target, so kind stays authoritative
until the final kind deletion. On a clean first run the pivot also WAITS for two
Flux-driven prerequisites instead of failing fast: the management `Cluster`
definition (polled up to `MGMT_READY_TIMEOUT`, surfacing failed Kustomizations
on timeout) and the first target nodes (up to 15m). A timeout in either prints
the Kustomization or node state and is safe to re-run. If a pivot phase fails:

1. Fix the reported cause.
2. Re-run the pivot (`scripts/toolbox-run.sh pivot`, or rerun bootstrap) from a
   checkout of the revision you want self-managed, normally `main`. A fallback
   native run must use the bootstrap context (`kind-mgmt`;
   `BOOTSTRAP_KUBECONTEXT` overrides). Toolbox mode selects kind's internal
   kubeconfig automatically. The CLI reuses an existing healthy `mgmt` kind
   cluster on rerun.
3. Set `PIVOT_SKIP_DELETE=1` to keep the kind bootstrap cluster around for
   inspection once the pivot completes.

> Note: never delete a moved `Cluster` or `Machine` object on the target
> cluster to work around a failure. The CAPI providers treat deletion as
> deprovisioning and destroy the real infrastructure (for CAPD, the local
> containers). Re-run the move instead. Only pure-config duplicates without a
> move hook (for example a Secret created by hand during a failed install) are
> safe to delete.

The management kubeconfig is written to `MGMT_KUBECONFIG`, with context
`kaipr-mgmt`. The toolbox mount makes its `/root/.kube/kaipr-mgmt.yaml` appear
on the host as `./.kube/kaipr-mgmt.yaml`; a fallback native run defaults to
`~/.kube/kaipr-mgmt.yaml`. After a toolbox `local-host` run on macOS the file
carries the `kind` Docker network address; see
[Host-side access after a toolbox local-host run (macOS)](#host-side-access-after-a-toolbox-local-host-run-macos)
for host-side use.

## Teardown

The wrapper runs the Rust subcommand in the toolbox:

```sh
TOOLBOX_IMAGE="$TOOLBOX_IMAGE" scripts/toolbox-run.sh teardown local-host
```

The retained `./teardown.sh` reference path and a native
`kaipr-bootstrap teardown [PROFILE]` run are the fallbacks. Teardown reads
resource names and targets from `bootstrap.toml` and discovers where the CAPI
controllers are running: the `mgmt` kind cluster before the pivot, or the
exported self-managed management kubeconfig after it.

The main controls keep the shell interface:

| Variable | Default | Effect |
|---|---|---|
| `FORCE_KIND_DELETE` | `0` | Literal `1` overrides the final controller-host deletion guard |
| `CLUSTER_DELETE_TIMEOUT` | `1200` seconds | CAPI cluster deletion wait |
| `PROVIDER_DELETE_TIMEOUT` | `300` seconds | CAPI provider deletion wait |
| `MGMT_KUBECONFIG` | `~/.kube/kaipr-mgmt.yaml` (in the toolbox that is the checkout's `.kube/` mount) | Post-pivot controller-host kubeconfig |

The hard preflight for `local-host` requires `kind` and `kubectl`. A
required-tool failure happens before mutation. The wrapper does not forward the
teardown controls in the table above; use a raw container invocation with
explicit `-e` entries or a fallback native run for recovery overrides.

For `local-host`, teardown suspends the workload Kustomization, deletes the
CAPD workload cluster, waits for its containers to disappear, removes either
the pre-pivot kind cluster or the post-pivot self-managed management
containers, and removes `kaipr-registry` last.

The controller-host guard prevents removal while CAPI workload deletion is
unconfirmed. Do not bypass it unless you accept orphaned containers.

## Validation

Run the repository validation before pushing:

```sh
mise run validate
```

The task checks shell syntax for the retained lifecycle scripts, runs the
`bootstrap.toml` manifest cross-check and the toolbox/doc tests, and builds
every kustomize overlay under `mgmt/` and `workload/`. It is repository
development, so it stays a host task on purpose (it needs the pinned Python,
`uv`, and `kubectl`). Contributors without a host toolchain can run the same
task in the toolbox, redirecting the uv environment off the mount:

```sh
docker run --rm -v "$PWD:/workspace" -w /workspace \
  -e MISE_AUTO_INSTALL=0 -e UV_PROJECT_ENVIRONMENT=/tmp/kaipr-venv \
  --entrypoint mise "$TOOLBOX_IMAGE" run validate
```

`.github/workflows/validate.yml` separately builds every overlay, runs the
Renovate managed-pin coverage tests, cross-checks `bootstrap.toml`, and lints
YAML on pushes to `main` and on pull requests.
`.github/workflows/bootstrap-rs.yml` runs Rust format, clippy, build, and tests,
then builds and smokes the toolbox image when its inputs change.
