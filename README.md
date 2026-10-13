# kaipr

kaipr is a reference that shows how to run a complete **AI platform** on a
single computer using Kubernetes and GitOps, with no cloud account, no GPU,
and no secrets. You run one command, and a Kubernetes cluster with an AI
inference gateway comes up from a single folder of YAML in Git.

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
  keys.

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
  credentials, no secrets in the tree.

## Quickstart

Build the toolbox image from the repository, then run the whole lifecycle:

```sh
docker build -f bootstrap-rs/Dockerfile -t kaipr-toolbox:dev .
export TOOLBOX_IMAGE=kaipr-toolbox:dev
cp .env.example .env        # no cloud credentials needed
scripts/toolbox-run.sh bootstrap
```

That one command starts the cluster, publishes this checkout to a local
registry, and reconciles the Flux instance and the AI platform from it. The
exported kubeconfig lands in `.kube/kaipr.yaml`. Watch it land:

```sh
export KUBECONFIG="$PWD/.kube/kaipr.yaml"
flux get kustomizations --watch
```

Once everything is Ready, send a chat request through the gateway to see the
AI path end to end. The simulator answers without any real model:

```sh
kubectl port-forward -n ai-platform svc/inference-gateway 18080:80
curl -s http://localhost:18080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"Qwen/Qwen3-32B","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'
```

Run the reference checks from the host:

```sh
mise run validate
```

Tear the whole stack down again with:

```sh
scripts/toolbox-run.sh teardown
```

## Going deeper

- [Inference platform](docs/inference.md) - the AI layer, the request path, and the vLLM overlay
- [Architecture](docs/architecture.md) - how the pieces fit
- [Operations](docs/operations.md) - toolbox runs, helper tasks, verification
- [Bootstrap CLI](docs/bootstrap-cli.md) - the `kaipr-bootstrap` engine and its knobs
- [Dependencies](docs/dependencies.md) - version surfaces and the update procedure
- [Extending the reference](docs/extending.md) - adding an app
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
│   └── inference-demo.py       # browser chat page in front of the gateway
├── workload/
│   └── local-host/             # the cluster's apps, delivered from Git
│       ├── ai-platform/        #   the AI inference platform
│       │   ├── crd/            #     CRDs: vendored gateway-api + agentgateway-crds
│       │   └── app/            #     agentgateway, model-server, inference, policies
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
