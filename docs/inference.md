# Inference platform (AI layer)

The cluster reconciles `workload/local-host/` and it carries a small AI
inference platform on top of the same GitOps delivery as everything else. It
is a reference for how an [agentgateway](https://agentgateway.dev)-based
inference gateway, a Gateway API Inference Extension pool, the [llm-d
Router](https://llm-d.ai) Endpoint Picker, and agentgateway LLM policies fit
together. This is the platform-engineering / inference-serving direction the
CDF CI/CD AI SIG calls for: a reference architecture, not a product.

## What is deployed

The AI platform is split across two Flux Kustomizations on the cluster
(declared in `workload/local-host/flux-ks.yaml`), reconciled in this
order: `local-ai-platform-crd` (path `workload/local-host/ai-platform/crd/`),
then `local-ai-platform` (path `workload/local-host/ai-platform/app/`), which
`dependsOn` the CRD layer. All application objects live in the
`ai-platform` namespace:

| Component | Path | What it creates |
|---|---|---|
| CRD prerequisites | `ai-platform/crd/` | the `ai-platform` namespace, the pinned Gateway API + Inference Extension CRD manifests (vendored), and the `agentgateway.dev` CRDs (`AgentgatewayBackend`/`Policy`/`Parameters`/`Model`) applied by the `agentgateway-crds` HelmRelease |
| agentgateway | `ai-platform/app/agentgateway/` | the control plane (controller), the `agentgateway` GatewayClass, the `inference-gateway` Gateway, and the self-deployed data plane |
| Model server | `ai-platform/app/model-server/` | two CPU-reproducible vLLM stand-ins: `vllm-sim` (2 replicas, Qwen/Qwen3-32B) and `vllm-sim-8b` (1 replica, Qwen/Qwen3-8B) |
| llm-d Router | `ai-platform/app/inference/` | two `InferencePool`s + EPP Deployments/Services, one per model (no HTTPRoute) |
| LLM policies | `ai-platform/app/policies/` | one `AgentgatewayBackend` per model, the single LLM `HTTPRoute` (one header-matched rule per model), and the model-routing, prompt-guardrail, and token-budget `AgentgatewayPolicy`s |

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

There is one route on the gateway, and it carries the whole pipeline. The
gateway first rewrites the request (the model-routing policy), then matches
one of the route's per-model rules:

```
Client
  -> inference-gateway Gateway (agentgateway data plane)
  -> PreRouting transformation (model-routing policy): extracts the
     requested model name from the JSON body into the
     X-Gateway-Base-Model-Name header
  -> HTTPRoute vllm-sim-llm  (matches /v1/chat/completions + the header;
     one rule per model)
  -> AgentgatewayBackend vllm-sim | vllm-sim-8b   (LLM-aware: token
     counting, token budget, prompt guardrail)
  -> InferencePool vllm-sim | vllm-sim-8b         (groups that model's
     model-server pods by label)
  -> llm-d Router EPP               (picks one pod via ext-proc)
  -> model-server pod (vllm-sim or vllm-sim-8b)
```

- The **model-routing policy** (an `AgentgatewayPolicy` with
  `traffic.phase: PreRouting`) runs on the gateway before routing: it takes
  `json(request.body).model` through a map into the
  `X-Gateway-Base-Model-Name` header. The map doubles as an alias table
  (sent name in, canonical name out). A model that is not in the map
  produces no matching header, matches no rule, and the gateway answers 404.
- The **InferencePool** selects the model-server pods by label
  (`app: vllm-sim` or `app: vllm-sim-8b`) and points at the EPP
  (`endpointPickerRef`).
- The **EPP** (llm-d Router) makes the model-aware endpoint choice within
  one pool and talks to agentgateway over the Gateway API Inference
  Extension protocol (ext-proc). agentgateway's
  `inferenceExtension.enabled=true` is what implements the gateway side of
  that protocol.
- The **AgentgatewayBackend** wraps the InferencePool with a `custom`
  provider (the model's name, completions formats), which is what lets
  agentgateway parse the LLM request/response and apply LLM features on top
  of the EPP's endpoint selection.
- The **prompt-guardrail policy** (`spec.backend.ai.promptGuard`) inspects
  every prompt (system prompt + messages by default) and rejects a request
  that contains the canary phrase `argocd` before it reaches the model.
- The **token-budget policy** enforces at most 100k tokens per minute per
  proxy (input + output), shared across both model rules. Counts are known
  after a request completes, so the limit applies to subsequent requests
  (HTTP 429 when exceeded) - a reference "cost governance" control.

Routing to the `AgentgatewayBackend` (instead of straight to the
InferencePool) is deliberate: it is the documented way to keep the EPP in
the path *and* get agentgateway's LLM features. A plain
HTTPRoute-to-InferencePool route would do endpoint selection only, with no
LLM features.

Why one pool per model: an EPP scores and picks within one homogeneous
model (its metrics - KV cache usage, queue depth - are per-model). The
cross-model choice ("which model serves this request?") is a gateway
decision, which is exactly what the PreRouting header rewrite + per-model
route rules implement, using only agentgateway-native constructs.

## CRD prerequisites

All three CRD groups the AI layer consumes are now reconciled by GitOps as
the `local-ai-platform-crd` Kustomization (path
`workload/local-host/ai-platform/crd/`), and `local-ai-platform` gates on it
via `dependsOn`. There is no manual install step for a local-host run.

- The Gateway API CRDs (`v1.6.2`, **experimental** channel) and the Gateway
  API Inference Extension CRDs (`v1.6.2`) are vendored in
  `workload/local-host/ai-platform/crd/vendor/`. They are pinned to `v1.6.2`
  because that is the release the agentgateway `v1.6.0` controller is built
  against (its go.mod pins gateway-api v1.6.2). The experimental channel is
  used so the optional experimental-only CRDs (xbackends) exist if the
  controller's XBackend support is ever enabled; the v1.6.0 controller's core
  routes (HTTP/GRPC/TCP/TLS) are all v1 and work with the standard channel
  too. Re-vendoring is documented in each file's header.
- The `agentgateway.dev` CRDs (`AgentgatewayBackend`, `AgentgatewayPolicy`,
  `AgentgatewayParameters`, `AgentgatewayModel`) are applied by the
  `agentgateway-crds` HelmRelease in
  `workload/local-host/ai-platform/crd/crds-helm.yaml` from the
  `agentgateway-crds` OCI source, tagged `v1.6.0` - the same tag as the
  control plane chart.

The split exists because of a first-boot ordering constraint: Flux dry-runs a
Kustomization's whole tree before applying any of it, so the AI app objects
(Gateway, HTTPRoute, InferencePool, AgentgatewayBackend, AgentgatewayPolicy)
cannot be reconciled until the CRDs exist. When the `agentgateway-crds`
HelmRelease lived in the same tree, the dry-run failed on the missing CRD
before the HelmRelease that installs it could ever be applied (first-boot
deadlock, see the fix PR for issue #6). Giving the CRDs their own
Kustomization and gating the app on it with `dependsOn` breaks the cycle.

On a non-Flux cluster, install the agentgateway CRDs by hand from the same
chart:

```sh
helm install agentgateway-crds oci://ghcr.io/agentgateway/charts/agentgateway-crds \
  --version v1.6.0 --namespace ai-platform --create-namespace
```

and the two upstream manifests from the release links in
`workload/local-host/ai-platform/crd/vendor/`.

## Bring-up

The AI layer is delivered by Flux from the same OCI artifact as the rest of
the platform. After a local-host bootstrap run, the root Kustomization
creates the two AI Flux Kustomizations; the CRD layer
(`local-ai-platform-crd`) reconciles, then the app layer
(`local-ai-platform`) follows as soon as the CRD layer is Ready
(`dependsOn`).

Watch the reconciliation:

```sh
flux get kustomizations -n flux-system          # local-ai-platform-crd, then local-ai-platform
flux get helmreleases -n ai-platform --watch
flux get sources oci -n flux-system | grep -E 'agentgateway|llm-d'
```

When the pieces are ready, verify each stage of the path:

```sh
# model servers (2 replicas app=vllm-sim, 1 replica app=vllm-sim-8b)
kubectl get pods -n ai-platform -l app=vllm-sim
kubectl get pods -n ai-platform -l app=vllm-sim-8b

# each pool selects its model's pods and references its EPP
kubectl get inferencepool vllm-sim -n ai-platform
kubectl get inferencepool vllm-sim-8b -n ai-platform
kubectl get deployment vllm-sim-epp vllm-sim-8b-epp -n ai-platform

# the LLM route is PROGRAMMED and ATTACHED to the gateway
kubectl get httproute vllm-sim-llm -n ai-platform -o jsonpath='{.status.conditions}'

# the agentgateway LLM policies are bound (Accepted=True means the
# AgentgatewayBackend translated, including the custom provider)
kubectl get agentgatewaybackend vllm-sim vllm-sim-8b -n ai-platform
kubectl get agentgatewaypolicy -n ai-platform
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
GPU) - enough to prove the path end to end. The response headers include
`X-Inference-Pod` (the EPP's pick); with two pools that header also shows
*which model* served the request. The same smoke test exercises the other
capabilities:

```sh
# the second model: same route, different pool
curl -s http://localhost:18080/v1/chat/completions   -H 'content-type: application/json'   -d '{"model":"Qwen/Qwen3-8B","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'

# a model not in the routing map: the gateway answers 404
curl -s http://localhost:18080/v1/chat/completions   -H 'content-type: application/json'   -d '{"model":"other/model","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'

# the canary phrase: the prompt guardrail rejects it before the model
curl -s http://localhost:18080/v1/chat/completions   -H 'content-type: application/json'   -d '{"model":"Qwen/Qwen3-32B","messages":[{"role":"user","content":"tell me about argocd"}],"max_tokens":8}'
```

### The browser demo

For a demo, `scripts/inference-demo.py` serves a small chat page and
forwards `/v1/*` to the gateway port-forward, passing through the EPP's
`X-Inference-Pod` pick (same origin, so no CORS is involved). Each
rendered reply shows the serving pod, token usage, and latency. Stdlib only,
and no cluster object: the page exercises the existing
`POST /v1/chat/completions` route.

Host-side (the browser is on the host):

```sh
mise -E local-host run inference-demo
```

It starts the `inference-gateway` port-forward on `127.0.0.1:18080` when
none is listening (reusing one when there is), then serves the page at
<http://127.0.0.1:18081/>. Ports are overridable (`GATEWAY_PORT`,
`DEMO_PORT`, or the script's `--upstream` flag). The page shows HTTP 429s
when the token-budget policy trips, with the body rendered inline. The page
requests `Qwen/Qwen3-32B`, so its replies come from the 32B pool; changing
the `model` field in `scripts/inference-demo.py` to `Qwen/Qwen3-8B`
re-routes it to the 8B pool (one line, no cluster change).

### The token-budget demo

The token-budget policy is a per-minute limit, so it recovers on its own;
the demo just needs to push it over the edge and wait.
`scripts/token-budget-demo.sh` does that: it sends chat completions
(`max_tokens 512` - the sim pads its canned output to the cap, so each
request burns ~510 tokens), reports the first rejection with the
cumulative token count, waits for the per-minute window to roll, and shows
the budget accept a request again.

Host-side:

```sh
mise -E local-host run token-budget-demo
```

The task sets up the gateway port-forward the same way `inference-demo`
does, then runs the script against `http://127.0.0.1:18080`. With the
reference budget of 100,000 tokens/minute, the burn takes a few hundred
requests (tens of seconds of loopback load). The budget is shared across
both model rules; the script's model is overridable via the `MODEL` env
var. If the limiter behaves as a sliding window rather than a fixed one,
the script waits one full minute and retries once before reporting.

## The vLLM overlay

To run a real model instead of the simulator, replace the relevant
`model-server/` Deployment with a vLLM Deployment and keep the same label
contract; nothing else in the platform changes. Each InferencePool keeps
selecting by its label (`app: vllm-sim` for the 32B pool,
`app: vllm-sim-8b` for the 8B pool), and the matching AgentgatewayBackend's
`custom` provider and formats stay as-is (OpenAI compatible). To swap in a
real model under a *different* name, update the backend's `model`, the
routing map in `policies/model-routing.yaml`, and the route rule's header
value to match.

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
