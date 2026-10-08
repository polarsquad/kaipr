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
four components, all in the `ai-platform` namespace:

| Component | Path | What it creates |
|---|---|---|
| Gateway API + Inference Extension CRDs | (operator prerequisite) | the `gateway.networking.k8s.io` and `inference.networking.k8s.io` CRDs |
| agentgateway | `agentgateway/` | the data-plane + control-plane, the `agentgateway` GatewayClass, and the `inference-gateway` Gateway |
| Model server | `model-server/` | a CPU-reproducible vLLM stand-in (`vllm-sim`) |
| llm-d Router | `inference/` | the `InferencePool` + EPP Deployment/Service (no HTTPRoute) |
| LLM policies | `policies/` | the `AgentgatewayBackend`, the single LLM `HTTPRoute`, and a token-budget `AgentgatewayPolicy` |

Pinned versions ( Renovate tracks the OCI/Git tags):

- Gateway API CRDs: `v1.6.2` (**experimental** channel)
- Gateway API Inference Extension CRDs: `v1.6.2`
- agentgateway: `v2.2.1` (chart `oci://ghcr.io/agentgateway/charts/agentgateway`)
- agentgateway CRDs: `0.0.0-alpha.a655af15` (`.../agentgateway-crds`)
- llm-d Router Gateway chart: `v0.9.0` (`oci://ghcr.io/llm-d/charts/llm-d-router-gateway`)
- Model server: `ghcr.io/llm-d/llm-d-inference-sim:v0.8.2`

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
# Gateway API: the experimental channel, not the standard one. The
# agentgateway controller registers informers for TCPRoute/TLSRoute at
# v1alpha2; the standard channel marks v1alpha2 served:false, so the
# controller never syncs and crash-loops. Experimental serves v1alpha2
# (deprecated) and matches the tested reference deployment.
kubectl apply -f "https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.6.2/experimental-install.yaml"
kubectl apply -f "https://github.com/kubernetes-sigs/gateway-api-inference-extension/releases/download/v1.6.2/manifests.yaml"
```

The `agentgateway.dev` CRDs (`AgentgatewayBackend`, `AgentgatewayPolicy`,
`AgentgatewayParameters`) come from the `agentgateway-crds` OCI source already
declared in `agentgateway/sources.yaml`. The `agentgateway` HelmRelease expects
them present, so apply that chart's CRDs (or the same source) before the
control plane starts:

```sh
helm repo add agentgateway https://cr.agentgateway.dev/charts
helm install agentgateway-crds agentgateway/agentgateway-crds   --version 0.0.0-alpha.a655af15 --namespace agentgateway-system --create-namespace
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

# the agentgateway LLM policies are bound
kubectl get agentgatewaybackend vllm-sim -n ai-platform
kubectl get agentgatewaypolicy vllm-sim-token-budget -n ai-platform
```

The `inference-gateway` Gateway has no `spec.addresses` (a laptop kind cluster
has no fixed IP). Read the resolved address back from status, then port-forward
the gateway for a host-side smoke test:

```sh
kubectl get gateway inference-gateway -n ai-platform   -o jsonpath='{.status.addresses[0].value}'
kubectl port-forward -n ai-platform svc/agentgateway 8080:8080
curl -s http://localhost:8080/v1/chat/completions   -H 'content-type: application/json'   -d '{"model":"Qwen/Qwen3-32B","messages":[{"role":"user","content":"hi"}],"max_tokens":8}'
```

The simulator returns a deterministic, model-shaped completion (no weights, no
GPU) - enough to prove the path end to end.

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
