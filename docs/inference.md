# Inference platform (AI layer)

The workload cluster in `workload/local-host/` carries a small AI inference
platform on top of the same GitOps delivery as everything else. It is a
reference for how an [agentgateway](https://agentgateway.dev)-based inference
gateway, a Gateway API Inference Extension pool, the [llm-d
Router](https://llm-d.ai) Endpoint Picker, and agentgateway LLM policies fit
together. This is the platform-engineering / inference-serving direction the
CDF CI/CD AI SIG calls for: a reference architecture, not a product.

## What is deployed

The `ai-platform` kustomization (`workload/local-host/ai-platform/`) reconciles
five components, all in the `ai-platform` namespace:

| Component | Path | What it creates |
|---|---|---|
| Gateway API + Inference Extension CRDs | (operator prerequisite) | the `gateway.networking.k8s.io` and `inference.networking.k8s.io` CRDs |
| agentgateway CRDs | `agentgateway/` | the `agentgateway.dev` CRDs (`AgentgatewayBackend`/`Policy`/`Parameters`/`Model`), applied by the `agentgateway-crds` HelmRelease |
| agentgateway | `agentgateway/` | the control plane (controller), the `agentgateway` GatewayClass, the `inference-gateway` Gateway, and the self-deployed data plane |
| Model server | `model-server/` | a CPU-reproducible vLLM stand-in (`vllm-sim`) |
| llm-d Router | `inference/` | the `InferencePool` + EPP Deployment/Service (no HTTPRoute) |
| LLM policies | `policies/` | the `AgentgatewayBackend`, the single LLM `HTTPRoute`, and a token-budget `AgentgatewayPolicy` |

Pinned versions (Renovate tracks the OCI/Git tags):

- Gateway API CRDs: `v1.6.2` (**experimental** channel)
- Gateway API Inference Extension CRDs: `v1.6.2`
- agentgateway: `v1.6.0` (chart `oci://ghcr.io/agentgateway/charts/agentgateway`)
- agentgateway CRDs: `v1.6.0` (`oci://ghcr.io/agentgateway/charts/agentgateway-crds`) - the tag MUST match the agentgateway chart
- llm-d Router Gateway chart: `v0.9.0` (`oci://ghcr.io/llm-d/charts/llm-d-router-gateway`)
- Model server: `ghcr.io/llm-d/llm-d-inference-sim:v0.8.2`

The agentgateway chart and CRDs are pinned to the same release on purpose.
The `custom` LLM provider on `AgentgatewayBackend` was added in v1.4.0; a
CRD/chart version skew (a CRD that exposes `spec.ai.provider.custom` paired
with a controller that cannot translate it) makes the backend land in
`Accepted=False / TranslationError: no supported LLM provider configured`.
The `v2.x` chart line is a separate kgateway-style deployment whose
self-deployed data plane is a legacy image and whose controller predates
custom-provider translation - do not pin it for an inference reference.

## The request path

There is one route on the gateway, and it carries the whole pipeline:

```
Client
  -> inference-gateway Gateway (agentgateway data plane)
  -> HTTPRoute vllm-sim-llm  (matches /v1/chat/completions)
  -> AgentgatewayBackend vllm-sim   (LLM-aware: token counting, token budget)
  -> InferencePool vllm-sim         (groups the model-server pods by label)
  -> llm-d Router EPP               (picks one pod via ext-proc)
  -> model-server pod (vllm-sim)
```

- The **InferencePool** selects the model-server pods by the label
  `app: vllm-sim` and points at the EPP (`endpointPickerRef`).
- The **EPP** (llm-d Router) makes the model-aware endpoint choice and talks
  to agentgateway over the Gateway API Inference Extension protocol
  (ext-proc). agentgateway's `inferenceExtension.enabled=true` is what
  implements the gateway side of that protocol.
- The **AgentgatewayBackend** wraps the InferencePool with a `custom` provider
  (model `Qwen/Qwen3-32B`, completions formats), which is what lets agentgateway
  parse the LLM request/response and apply LLM features on top of the EPP's
  endpoint selection.
- The **token-budget policy** enforces at most 100k tokens per minute per
  proxy (input + output). Counts are known after a request completes, so the
  limit applies to subsequent requests (HTTP 429 when exceeded) - a reference
  "cost governance" control.

Routing to the `AgentgatewayBackend` (instead of straight to the InferencePool)
is deliberate: it is the documented way to keep the EPP in the path *and* get
agentgateway's LLM features. A plain HTTPRoute-to-InferencePool route would do
endpoint selection only, with no LLM features.

## CRD prerequisites

The Gateway API and Inference Extension CRDs are cluster-wide prerequisites,
installed once per cluster by the platform operator - they are not workload
objects this kustomization owns. Install them before the AI layer reconciles:

```sh
# Gateway API: v1.6.2, the same release the agentgateway v1.6.0 controller
# is built against (its go.mod pins gateway-api v1.6.2). The experimental
# channel is used so the optional experimental-only CRDs (xbackends) exist
# if the controller's XBackend support is ever enabled; the v1.6.0
# controller's core routes (HTTP/GRPC/TCP/TLS) are all v1 and work with the
# standard channel too.
kubectl apply -f "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.6.2/experimental-install.yaml"
kubectl apply -f "https://github.com/kubernetes-sigs/gateway-api-inference-extension/releases/download/v1.6.2/manifests.yaml"
```

The `agentgateway.dev` CRDs (`AgentgatewayBackend`, `AgentgatewayPolicy`,
`AgentgatewayParameters`, `AgentgatewayModel`) are applied by the
`agentgateway-crds` HelmRelease in `agentgateway/helm.yaml` from the
`agentgateway-crds` OCI source in `agentgateway/sources.yaml` - the same
`v1.6.0` tag as the control plane chart. Because that HelmRelease is in the
same kustomization, Flux orders it before the `agentgateway` control plane.
On a non-Flux cluster, install them by hand from the same chart:

```sh
helm install agentgateway-crds oci://ghcr.io/agentgateway/charts/agentgateway-crds \
  --version v1.6.0 --namespace ai-platform --create-namespace
```

## Bring-up

The AI layer is delivered by Flux from the same OCI artifact as the rest of the
workload cluster (there is no separate Flux Kustomization per component, the
same model as `podinfo`). After a local-host bootstrap run, the whole
`workload/local-host/` tree - `podinfo` plus `ai-platform` - reconciles.

Watch the reconciliation:

```sh
flux get helmreleases -n ai-platform --watch
flux get sources oci -n flux-system | grep -E 'agentgateway|llm-d'
```

When the pieces are ready, verify each stage of the path:

```sh
# model servers (2 replicas, labeled app=vllm-sim)
kubectl get pods -n ai-platform -l app=vllm-sim

# the pool selects them and references the EPP
kubectl get inferencepool vllm-sim -n ai-platform
kubectl get deployment vllm-sim-epp -n ai-platform

# the LLM route is PROGRAMMED and ATTACHED to the gateway
kubectl get httproute vllm-sim-llm -n ai-platform -o jsonpath='{.status.conditions}'

# the agentgateway LLM policies are bound (Accepted=True means the
# AgentgatewayBackend translated, including the custom provider)
kubectl get agentgatewaybackend vllm-sim -n ai-platform
kubectl get agentgatewaypolicy vllm-sim-token-budget -n ai-platform
```

The `inference-gateway` Gateway has no `spec.addresses` on a laptop kind
cluster (the controller's LoadBalancer service has no public IP). The data
plane is the controller-self-deployed `inference-gateway` Deployment, and its
`inference-gateway` Service (LoadBalancer, port 80) carries the listener.
Port-forward that service for a host-side smoke test:

```sh
kubectl port-forward -n ai-platform svc/inference-gateway 18080:80
curl -s http://localhost:18080/v1/chat/completions   -H 'content-type: application/json'   -d '{"model":"Qwen/Qwen3-32B","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'
```

The simulator returns a deterministic, model-shaped completion (no weights, no
GPU) - enough to prove the path end to end.

### The browser demo

For a demo, `scripts/inference-demo.py` serves a small chat page and
forwards `/v1/*` to the gateway port-forward, passing through the EPP's
`X-Inference-Pod` pick (same origin, so no CORS is involved). Each
rendered reply shows the serving pod, token usage, and latency. Stdlib only,
and no cluster object: the page exercises the existing
`POST /v1/chat/completions` route.

Host-side, like `podinfo-port-forward` (the browser is on the host):

```sh
mise -E local-host run inference-demo
```

It starts the `inference-gateway` port-forward on `127.0.0.1:18080` when
none is listening (reusing one when there is), then serves the page at
<http://127.0.0.1:18081/>. Ports are overridable (`GATEWAY_PORT`,
`DEMO_PORT`, or the script's `--upstream` flag). The page shows HTTP 429s
when the token-budget policy trips, with the body rendered inline.

## The vLLM overlay

To run a real model instead of the simulator, replace `model-server/` with a
vLLM Deployment and keep the same label contract; nothing else in the platform
changes. The InferencePool keeps selecting by `app: vllm-sim`, and the
AgentgatewayBackend's `custom` provider and formats stay as-is (OpenAI
compatible).

```yaml
# model-server/vllm.yaml (replaces the simulator Deployment; same label)
apiVersion: apps/v1
kind: Deployment
metadata:
  name: vllm
  namespace: ai-platform
spec:
  replicas: 1
  selector:
    matchLabels: { app: vllm-sim }
  template:
    metadata:
      labels:
        app: vllm-sim
        inference.networking.k8s.io/engine-type: vllm
    spec:
      containers:
        - name: vllm
          image: vllm/vllm-openai:latest
          args:
            - --model
            - Qwen/Qwen3-32B
          ports:
            - containerPort: 8000
          resources:
            limits:
              nvidia.com/gpu: 1
```

Because the label and port match the InferencePool, the EPP, the
AgentgatewayBackend, and the LLM route all keep working unchanged. This is the
"simulated now, real later" split the reference is built around.

## What is intentionally out of scope

- **GPU scheduling / quota** - the reference runs CPU-only (simulator); the vLLM
  overlay documents the GPU step without provisioning one.
- **Model registry / training / evaluation pipelines** - the CDF AI SIG's
  MLOps-continuous-training and agent-harness scope; the SIG's non-goals
  include not picking model providers.
- **Secrets** - none are in-tree. The OpenAI-compatible path needs no API key;
  if you point at an external model, inject the key as a secret referenced by
  the backend, never as a literal.
