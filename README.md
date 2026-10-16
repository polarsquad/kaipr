# kaipr

kaipr is a reference that shows how to run a complete **AI platform** on a
single computer using Kubernetes and GitOps, with no cloud account, no GPU,
and no plaintext secrets: the tree carries credentials only as SOPS-encrypted
`*.sops.yaml` manifests under `workload/` ([Secret management](docs/secrets.md)).
You run one command, and a Kubernetes cluster with an AI inference gateway
comes up from a single folder of YAML in Git.

It has two halves in one repository:

- **A lean base.** A local `kind` cluster running Flux, all in containers on
  your machine. This scaffolding is derived from
  [krops](https://github.com/polarsquad/krops), an open-source tool for
  GitOps-managed Kubernetes clusters: the bootstrap engine, the toolbox, and
  the repository layout are krops' here, scoped down to one environment.
- **An AI platform built on [agentgateway](https://agentgateway.dev).** A
  gateway that routes AI chat requests to a model, picks the best model server
  for each request, and enforces token budgets. The model runs as a CPU
  simulator by default, so you can try the whole thing with no hardware or API
  keys. An optional second route sends Anthropic Messages API requests to the
  hosted Anthropic API; its key is the one SOPS-encrypted credential in the
  tree ([Secret management](docs/secrets.md)).

You do not need Kubernetes experience to follow the quickstart, but the
machine does need a way to run [Docker](https://www.docker.com/) (or
[Podman 5.5+](https://podman.io/)).

A complete end-to-end walkthrough of the demo, from bootstrap to the live
chat page, is in [QUICKSTART.md](QUICKSTART.md).

## What you get

- One local Kubernetes cluster on your host, running the whole platform in
  Docker containers.
- An AI inference gateway in front of the model, with per-request routing and
  a token budget.
- One Git repository as the source of truth: change a file, push it, and the
  cluster converges to match. No second toolchain, no state file, no cloud
  credentials, and no plaintext secrets in the tree: SOPS-encrypted
  `*.sops.yaml` manifests under `workload/` are the only secret-bearing files
  ([Secret management](docs/secrets.md)).

## Quickstart

Only pre-requisites are Docker/Podman. Ensure you're at the cloned/forked
repository root to run all commands. Will require two terminal sessions.

### Step 1: Bootstrap the local kaipr cluster

```sh
docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
export TOOLBOX_IMAGE=kaipr-toolbox:dev
scripts/toolbox-run.sh bootstrap
```

### Step 2: Change your context and confirm kaipr node exists

```sh
export KUBECONFIG="$PWD/.kube/kaipr.yaml" && kubectl get nodes
```

### Step 3: If you see a kaipr node in the above output, run

```sh
mise -E local-host run inference-demo
```

The demo is now live at `http://127.0.0.1:18081`; it and the port-forward
stays up until you Ctrl-C the terminal running or kill the processes.

## Going deeper

- [Inference platform](docs/inference.md) - the AI layer, the request path, and the vLLM overlay
- [Architecture](docs/architecture.md) - how the pieces fit
- [Operations](docs/operations.md) - toolbox runs, helper tasks, verification
- [Bootstrap CLI](docs/bootstrap-cli.md) - the `kaipr-bootstrap` engine and its knobs
- [Dependencies](docs/dependencies.md) - version surfaces and the update procedure
- [Extending the reference](docs/extending.md) - adding an app
- [Secret management](docs/secrets.md) - the SOPS + age machinery, key lifecycle, and rotation
- [PR review: konflate](docs/konflate.md) - rendering Flux resources for review

## Repository layout

```
.
├── QUICKSTART.md               # end-to-end demo walkthrough
├── bootstrap.toml              # environment + chart config for kaipr-bootstrap
├── bootstrap.sh                # local-host lifecycle: kind, Flux seed, reconcile
├── teardown.sh                 # reverse-order cleanup
├── bootstrap-common.sh         # shared helpers
├── bootstrap-rs/               # the generic Rust bootstrap engine (kaipr-bootstrap)
│   ├── Dockerfile              # the toolbox image (mise + podman remote client)
│   └── src/
├── scripts/
│   ├── toolbox-run.sh          # Docker/Podman wrapper for the toolbox
│   ├── inference-demo.py       # browser chat page in front of the gateway
│   └── token-budget-demo.sh    # burns the token budget to show the 429 path
├── workload/
│   └── local-host/             # the cluster's apps, delivered from Git
│       ├── ai-platform/        #   the AI inference platform
│       │   ├── crd/            #     CRDs: vendored gateway-api + agentgateway-crds
│       │   └── app/            #     agentgateway, model-server, inference, policies, secrets
│       └── flux-ks.yaml        #   the two Flux Kustomizations (app dependsOn crd)
├── tests/                      # self-contained cross-checks (run by mise validate)
├── docs/
└── .github/workflows/          # validate, bootstrap-rs, konflate, toolbox-release
```

## Validation

`mise run validate` (and the `validate` GitHub Action) gate every change:
shell scripts parse, the `bootstrap.toml` cross-checks the imperative chart
pins, every kustomization under `workload/` builds, and the toolbox / OCI
publication / docs cross-checks pass. The Rust engine has its own `fmt` /
`clippy` / `test` gate.

## License

Apache-2.0, like krops. See [LICENSE](LICENSE).
