# kaipr
## Kubernetes AI Platform Reference

![kaipr logo](docs/kaipr-logo.svg)

kaipr is a GitOps pattern for managing infrastructure through the Kubernetes
API with plain declarative YAML. Desired state is stored as Kubernetes
resources in Git; [Flux](https://fluxcd.io/) delivers those resources and
controllers continuously reconcile the infrastructure to match them. One API,
one RBAC model, one audit trail for infrastructure and workloads. No HCL, no
state file, no second toolchain.

This reference does two things in one repository:

- A **lean, laptop-reproducible base**: a disposable [kind](https://kind.sigs.k8s.io/)
  bootstrap cluster that [Cluster API](https://cluster-api.sigs.k8s.io/) +
  [CAPD](https://cluster-api.sigs.k8s.io/cluster-api/docs/capd) pivots into a
  self-managed management cluster and a workload cluster, all on one host.
- An **AI inference platform** reconciled onto the workload cluster from the
  same Git source: an [agentgateway](https://agentgateway.dev) inference
  gateway in front of a Gateway API Inference Extension `InferencePool` and the
  [llm-d Router](https://llm-d.ai) Endpoint Picker, with a CPU-reproducible
  model server and LLM-aware agentgateway policies (token budgets). See
  [Inference platform](docs/inference.md).

It follows the platform-engineering / inference-serving direction of the
[CDF CI/CD AI SIG](https://github.com/cdfoundation/AI): an open,
vendor-neutral reference a fork can adapt, not a product. Fork it, strip it
down, and make it yours.

## Who this is for

Platform engineers who already run Kubernetes and want a working reference for
running a GitOps-managed cluster on a single host *and* standing up an
inference gateway with LLM routing and token-budget controls, without cloud
credentials or GPUs.

## Prerequisites

- Docker or Podman 5.5+ (kind, the local registry, and the toolbox run on it).
- macOS or Linux host. A host that routes the `kind` Docker network directly
  (Linux) needs no extra steps; macOS needs one kubeconfig rewrite (below).
- Git. Everything else (Flux, clusterctl, kind, helm, kubectl, sops, age,
  uv, ruff) is pinned by `mise` and installed on demand.

There are no cloud accounts, API keys, or GPUs for the default path. The model
server is a CPU simulator; the [vLLM overlay](docs/inference.md#the-vllm-overlay)
documents the GPU step.

## Quickstart

Build the toolbox image from the repository, then run the complete local-host
lifecycle. `scripts/toolbox-run.sh` handles the container mounts, loads `.env`,
and persists kubeconfigs under `.kube/`.

```sh
docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
export TOOLBOX_IMAGE=kaipr-toolbox:dev
mkdir -p .kube
cp .env.example .env        # local-host: no cloud credentials needed
scripts/toolbox-run.sh bootstrap
```

The lifecycle boots the kind bootstrap cluster, starts a local registry,
publishes the initial OCI artifact from this checkout (the toolbox runs the
`oci-push` helper task), seeds Flux, and pivots to a self-managed management
and workload cluster. After the workload cluster reconciles, both `podinfo`
and the `ai-platform` tree come up from the same artifact.

Watch progress:

```sh
export KUBECONFIG="$PWD/.kube/kaipr-mgmt.yaml"
flux get kustomizations --watch
```

Run the reference checks from the host:

```sh
mise run validate
```

On macOS, a local-host run leaves the exported management kubeconfig pointing
at the `kind` Docker network, which Docker Desktop does not route; rewrite a
host copy before the `export` above. Linux hosts route the `kind` network
directly and need no rewrite. See
[Host-side access after a toolbox local-host run (macOS)](docs/operations.md#host-side-access-after-a-toolbox-local-host-run-macos).

Tear down the whole stack (same mounts, `teardown` subcommand):

```sh
scripts/toolbox-run.sh teardown
```

## The AI platform

`workload/local-host/ai-platform/` reconciles the inference platform onto the
workload cluster:

- **agentgateway** (v1.6.0) with `inferenceExtension.enabled=true`, the
  `agentgateway` GatewayClass, and the `inference-gateway` Gateway.
- **Gateway API Inference Extension**: an `InferencePool` that selects the
  model-server pods by label, plus the **llm-d Router** EPP (v0.9.0) that
  picks a pod per request over ext-proc.
- A **CPU-reproducible model server** (`ghcr.io/llm-d/llm-d-inference-sim`) that
  speaks the vLLM API without weights or GPUs, with a documented vLLM overlay.
- **agentgateway LLM policies**: an `AgentgatewayBackend` wrapping the
  InferencePool, the single LLM `HTTPRoute`, and a token-budget
  `AgentgatewayPolicy` (100k tokens/minute/proxy).

The full request path, the CRD prerequisites, bring-up verification, and the
vLLM overlay are in [Inference platform](docs/inference.md).

## The bootstrap CLI

The Rust [`kaipr-bootstrap`](docs/bootstrap-cli.md) binary is a generic
bootstrap engine: it reads the repository's `bootstrap.toml` for the
environment list, chart pins, and per-environment teardown, and handles
bootstrap, pivot, and teardown. This reference declares a single environment,
`local-host` (the `bootstrap.toml` is the single source of truth for what
environments exist). The lifecycle scripts wrap it; see
[Bootstrap CLI](docs/bootstrap-cli.md) for the knob table and entry-point
status.

## Documentation

- [Inference platform](docs/inference.md) - the AI layer, request path, and vLLM overlay
- [Architecture](docs/architecture.md) - how the pieces fit
- [Operations](docs/operations.md) - toolbox runs, helper tasks, verification
- [Bootstrap CLI](docs/bootstrap-cli.md) - the `kaipr-bootstrap` engine and knobs
- [Secrets](docs/secrets.md) - SOPS/age for in-tree secrets
- [Dependencies](docs/dependencies.md) - version surfaces and the update procedure
- [Adding clusters and apps](docs/extending.md) - extending the reference
- [PR review: konflate](docs/konflate.md) - rendering Flux resources for review

## Repository layout

```
.
├── bootstrap.toml              # environment + chart config for kaipr-bootstrap
├── bootstrap.sh                # local-host lifecycle: seed Flux, pivot, reconcile
├── pivot.sh                    # pivot a bootstrap cluster to self-managed
├── teardown.sh                 # reverse-order cleanup
├── bootstrap-common.sh         # shared helpers
├── bootstrap-rs/               # the generic Rust bootstrap engine (kaipr-bootstrap)
│   ├── Dockerfile              # the toolbox image (mise + podman remote client)
│   └── src/
├── scripts/
│   └── toolbox-run.sh          # Docker/Podman wrapper for the toolbox
├── mgmt/
│   └── local-host/             # management cluster: CAPI providers, CAPD, Flux apps
│       ├── capi-providers/     #   cluster-api, capd, cluster-api-addon-provider
│       ├── infrastructure/     #   flux-operator, cert-manager, capi-operator
│       ├── addons/             #   flux-apps, cni
│       └── clusters/           #   management + docker workload cluster class
├── workload/
│   └── local-host/             # workload cluster apps, delivered from Git
│       ├── podinfo/            #   the reference app (smoke test)
│       └── ai-platform/        #   the AI inference platform
│           ├── agentgateway/   #     agentgateway + Gateway + sources
│           ├── model-server/   #     CPU vLLM simulator
│           ├── inference/      #     InferencePool + llm-d Router EPP
│           └── policies/       #     AgentgatewayBackend, LLM route, token budget
├── tests/                      # self-contained cross-checks (run by mise validate)
├── docs/
└── .github/workflows/          # validate, bootstrap-rs, konflate, toolbox-release
```

## Validation

`mise run validate` (and the `validate` GitHub Action) gate every change:
shell scripts parse, the `bootstrap.toml` cross-checks the imperative chart
pins against the Flux-reconciled versions, every kustomization under `mgmt/`
and `workload/` builds, and the toolbox / kubeconfig / docs cross-checks pass.
The Rust engine has its own `fmt` / `clippy` / `test` gate.

## License

Apache-2.0, like krops. See [LICENSE](LICENSE).
