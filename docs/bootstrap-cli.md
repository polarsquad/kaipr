# The bootstrap CLI (`kaipr-bootstrap`)

The imperative part of kaipr lives in one Rust binary under
[`bootstrap-rs/`](../bootstrap-rs/). It implements the one-time bootstrap and
the teardown. After bootstrap finishes, Flux owns the declared state until
teardown.

The binary is the generic bootstrap engine carried over from
[krops](https://github.com/polarsquad/krops). It is deliberately environment
agnostic: it reads everything repo-specific from
[`bootstrap.toml`](../bootstrap.toml). This reference enables exactly one
environment, `local-host`, so the whole thing runs on a laptop with no cloud
account and no secrets.

The binary is a behavioral port of `bootstrap.sh` and `teardown.sh`. It
preserves their step order, progress messages, environment interface, and
safety guards, with two deliberate upgrades:

- **Reruns are safe by default.** An existing healthy `kaipr` kind cluster is
  reused and each bootstrap step is idempotent. Pass `--recreate` to delete
  and rebuild the kind cluster instead.
- **Typed process execution.** Tool arguments are passed as argv entries, and
  the registry HTTP check uses reqwest with explicit timeouts.

## Distribution and build

The toolbox image, `ghcr.io/polarsquad/kaipr-toolbox`, contains
`kaipr-bootstrap` and the pinned tools required by the lifecycle. It is built
from `bootstrap-rs/Dockerfile`; there is no published release yet. See
[Operations](./operations.md) for the container invocation and host runtime
contract.

The `toolbox-release` workflow runs on `v*` tags. It requires the tag to match
`bootstrap-rs/Cargo.toml`, builds Linux amd64 and arm64 images each on a
runner of that architecture, publishes `X.Y.Z`, `X.Y`, and stable `latest`
tags, signs the image with GitHub OIDC, and attaches a Syft SPDX JSON SBOM
attestation. The `bootstrap-rs` CI workflow also builds and smokes the arm64
image when its inputs change.

Each architecture builds natively rather than under QEMU: emulating the
amd64 rustc binary on the arm64 runner segfaults deterministically. The two
single-arch images are merged into the multi-arch manifest with
`docker buildx imagetools create` in plain bash rather than
`docker/metadata-action`, whose `tags` output can collapse to a single tag
and silently drop the `X.Y` and `latest` aliases.

Verify a published image against the workflow's OIDC identity:

```sh
cosign verify ghcr.io/polarsquad/kaipr-toolbox:X.Y.Z \
  --certificate-identity-regexp \
    '^https://github.com/polarsquad/kaipr/.github/workflows/toolbox-release.yml@refs/tags/vX.Y.Z$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

No release has been published yet: the workflow runs only when a `v*` tag is
pushed. Until then, build the current checkout as shown in
[Operations](./operations.md). A published release will carry the stable tags
described above.

Build the CLI directly for native development:

```sh
cd bootstrap-rs
cargo build --locked
cd ..
./bootstrap-rs/target/debug/kaipr-bootstrap --help
./bootstrap-rs/target/debug/kaipr-bootstrap teardown --help
```

Run the binary from the repository root so its default `./bootstrap.toml` path
resolves. Set `BOOTSTRAP_CONFIG` when running from another directory.

CI runs `cargo fmt --check`, clippy with warnings denied, a locked build, and
the test suite. The toolchain is pinned in `bootstrap-rs/rust-toolchain.toml`;
crate dependencies are locked in `Cargo.lock`.

## Interface

```text
kaipr-bootstrap [OPTIONS] [PROFILE] [COMMAND]
kaipr-bootstrap teardown [PROFILE]
```

Common examples:

```sh
kaipr-bootstrap                         # local-host bootstrap
kaipr-bootstrap --recreate local-host   # rebuild the kind cluster
kaipr-bootstrap teardown                # local-host teardown
kaipr-bootstrap teardown local-host     # local-host teardown (explicit)
```

- `PROFILE` is the CLI's positional name. Its value names a section under
  `[environments.*]` in [`bootstrap.toml`](../bootstrap.toml). This reference
  checks in a single environment, `local-host`.
- A non-empty `KAIPR_PROFILE` overrides the positional profile. If neither is
  set, `bootstrap.default-environment` from `bootstrap.toml` is used.
- `--recreate` applies to bootstrap only. Teardown is a subcommand and keeps
  its script-compatible controls in environment variables.
- There is no pivot: the single kind cluster is the only cluster, so there is
  nothing to move. Rerunning the normal command resumes an interrupted
  bootstrap.

## Repository configuration

Repository-owned cluster names, paths, and chart versions live in
[`bootstrap.toml`](../bootstrap.toml):

- `[bootstrap]` carries the global names: `default-environment`,
  `kind-cluster` (`kaipr`), `kind-context` (`kind-kaipr`), `registry-name`
  (`kaipr-registry`), and `flux-namespace` (`flux-system`).
- `[charts]` carries the imperative chart pins. `local-host` has one,
  `flux-operator`, installed by helm before the `FluxInstance` is seeded.
- `[environments.<name>]` declares each environment: `kind` (must match the
  section name), `sync` (the Flux sync source; `oci` only in this reference),
  and `sync-path` (the artifact root path the `FluxInstance` reconciles).

The binary retains generic fallback defaults and sequence-level contracts. It
reads `./bootstrap.toml` by default; `BOOTSTRAP_CONFIG` selects another path.

Runtime environment variables take precedence where an override exists.
`mise run validate` parses the file and cross-checks its chart pins and
environment sections against the Git tree. Renovate updates the annotated
chart pins. See [Dependencies](./dependencies.md).

The `local-host` entry declares `sync = "oci"`: the Flux instance pulls the
OCI artifact the local registry serves, with `sync-path = workload/local-host`.

## Bootstrap controls

| Variable | Default | Used by |
|---|---|---|
| `BOOTSTRAP_CONFIG` | `./bootstrap.toml` | Repository configuration path |
| `KAIPR_PROFILE` | positional profile, then `bootstrap.default-environment` (`local-host` checked in) | Environment selection |
| `REGISTRY_PORT` | `5001` | Local-host registry host port |
| `REGISTRY_READY_RETRIES` | `120` | Local-host registry readiness attempts |
| `LOCAL_RECONCILE_TIMEOUT` | `15m` | Local-host reconciliation waits |
| `CONTAINER_ENGINE` | auto-detect Docker, then Podman | kind and registry engine |
| `ENGINE_SOCK` | resolved by the toolbox wrapper | Daemon-side engine socket |
| `KAIPR_KUBECONFIG` | `~/.kube/kaipr.yaml` (in the toolbox, the checkout's `.kube/kaipr.yaml`) | Exported host kubeconfig |
| `OCI_REPOSITORY` / `OCI_TAG` | `kaipr` / `latest` | Local-host OCI artifact name |

### Toolbox runtime contracts

The toolbox runtime adds four contracts:

- `KAIPR_TOOLBOX=1` enables internal kind networking: after creating (or
  reusing) the cluster the CLI joins the `kind` network, so the internal API
  endpoint and `kaipr-registry:5000` resolve by name.
- `ENGINE_SOCK` names the engine socket path as seen by the daemon. The wrapper
  resolves it for Docker Desktop, Docker contexts, rootful or rootless Podman,
  and `podman machine`.
- `KUBECONFIG` must name one writable file, not a colon-separated list. The
  wrapper uses `/workspace/.kube/kind.yaml`.
- `CONTAINER_HOST` points the Podman remote client at the mounted engine
  socket (`unix:///var/run/docker.sock`) when `KAIPR_TOOLBOX=1`. The CLI sets
  it only if unset, before any Podman probe, so an operator-supplied value is
  kept regardless of the eventual `CONTAINER_ENGINE`.

The container reaches the local registry at `kaipr-registry:5000`. Its
`/root/.kube` mount makes the exported kubeconfig persist on the host as
`./.kube/kaipr.yaml`. The wrapper's environment allowlist and override
limitations are documented in [Operations](./operations.md#toolbox-container-primary-interface).

**Design notes (`bootstrap-rs/src/engine.rs`):** engine detection, socket
resolution, `CONTAINER_HOST` defaulting, and kind-network attach/detach used to
be implemented separately in `preflight_checks`, a `detect_engine_for_network`
helper that read `Config` defaults instead of the engine `preflight_checks`
actually resolved, `teardown::detect_engine`, and the toolbox shell entrypoint
(which also detected and validated the engine before Rust ever ran). All of
that now lives in one module: `preflight_checks` resolves the engine once and
threads it through bootstrap and teardown instead of re-detecting it, and
`toolbox-entrypoint.sh` only sets `KAIPR_TOOLBOX=1` and execs. Inside the
toolbox, `podman info` reports the mounted *client* socket (`CONTAINER_HOST`),
not the daemon-side path kind's `extraMounts` need, so the module skips
querying it there and uses the static fallback instead. kind-network
attach/detach checks actual network membership rather than matching Docker's
and Podman's differently worded "already connected" errors, so it doesn't
depend on engine- or version-specific error text.

## What bootstrap does (local-host)

1. **Preflight:** validate the environment and required tools, and select a
   running container engine. The base set is `kind`, `helm`, `kubectl`, and
   `mise`; the OCI sync surface adds `flux` (the error stream) and `curl` (the
   `oci-push` task's registry probe). A preflight failure happens before any
   mutation.
2. **Kind and registry:** create or reuse the `kaipr` kind cluster (config
   carries the containerd mirror from the published host port to
   `kaipr-registry:5000` and the engine-socket mount), then start or reuse the
   local registry container on the kind network.
3. **Publish:** run `mise -E <profile> run oci-push`, which packages
   `workload/local-host/` and pushes it to
   `oci://kaipr-registry:5000/<OCI_REPOSITORY>:<OCI_TAG>` over the kind
   network.
4. **Seed Flux:** install the `flux-operator` chart by helm (version pinned in
   `bootstrap.toml`), then apply the `FluxInstance` in `flux-system` with the
   OCI sync, `sync.path = workload/local-host`, and the insecure-registry
   patch.
5. **Watch reconciliation:** follow the Flux controller error stream until the
   root Kustomization and its CRD-layer parent are Ready (each wait bounded by
   `LOCAL_RECONCILE_TIMEOUT`).
6. **Export the kubeconfig:** write `.kube/kaipr.yaml` via
   `kind get kubeconfig --name kaipr` (chmod 600). kind publishes the API port
   on localhost, so the file works from the host with no rewrite.

If a phase fails, fix the cause and rerun the same command. Every step is
idempotent and kind remains the running cluster throughout, so a rerun is
always safe.

## Teardown controls and behavior

```sh
kaipr-bootstrap teardown [PROFILE]
```

| Variable | Default | Effect |
|---|---|---|
| `CONTAINER_ENGINE` | auto-detect Docker, then Podman | kind and registry engine |
| `ENGINE_SOCK` | resolved by the toolbox wrapper | Daemon-side engine socket |
| `KAIPR_KUBECONFIG` | `~/.kube/kaipr.yaml` (in the toolbox, the checkout's `.kube/kaipr.yaml`) | Exported host kubeconfig to remove |

Teardown checks required tools before mutation: it needs a running container
engine to remove the registry and the cluster.

In the toolbox the run first joins the kind network so the internal kind
endpoint resolves before the steps run. Teardown addresses the kind cluster by
explicit name, so a stale kubectl current-context cannot redirect the
destructive operation.

For `local-host` the sequence is, in reverse creation order:

1. Remove the exported host kubeconfig (a missing file is a clean no-op).
2. Remove the local registry container `kaipr-registry` (it is attached to the
   `kind` network, so it goes before the cluster).
3. Delete the `kaipr` kind cluster and its containers.

Each step is idempotent: a missing resource is skipped, so an interrupted run
can be resumed by rerunning. There is no CAPI inventory, management cluster, or
cloud resource to unwind.

## Entry-point and parity status

`scripts/toolbox-run.sh bootstrap` and `scripts/toolbox-run.sh teardown`
invoke this CLI in the toolbox container.

The shell scripts (`bootstrap.sh`, `teardown.sh`) remain as native reference
and fallback paths. Local-host bootstrap and teardown have completed parity
runs against them.
