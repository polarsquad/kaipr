#!/usr/bin/env python3
"""Cross-check bootstrap.toml against the Git tree.

The single-cluster reference has one imperative chart pin (flux-operator)
that the bootstrap installs before seeding Flux; no Flux Kustomization
reconciles that chart, so the pin must not also appear as a HelmRelease in
the workload tree (it would drift against the imperative install). The rest
of the cross-check is structural: the file parses, the default environment
exists, every environment's kind matches its section name, its sync source
is the only supported one, and its sync path is a directory in the repo.
Requires Python 3.11+ (tomllib); mise and CI both provide it.
"""

import re
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# HelmRelease blocks: the chart name is the value of spec.chart.spec.chart
# (or spec.chart for a bare string form). We only need to know which charts
# Flux would reconcile from the workload tree.
HR_CHART_RE = re.compile(r"^kind:\s*HelmRelease\s*$", re.MULTILINE)
CHART_VALUE_RE = re.compile(r"^\s*chart:\s*(?P<chart>\S+)\s*$", re.MULTILINE)
NAMESPACE_RE = re.compile(r"^\s*namespace:\s*(?P<ns>\S+)\s*$", re.MULTILINE)


def helmrelease_charts(root: Path) -> dict[str, str]:
    """Map chart name -> manifest path for every HelmRelease under root."""
    charts: dict[str, str] = {}
    for manifest in sorted(root.rglob("*.yaml")):
        text = manifest.read_text(encoding="utf-8")
        if not HR_CHART_RE.search(text):
            continue
        for document in text.split("\n---\n"):
            if "kind: HelmRelease" not in document:
                continue
            match = CHART_VALUE_RE.search(document)
            if not match:
                continue
            charts.setdefault(match.group("chart"), str(manifest.relative_to(REPO_ROOT)))
    return charts


def main() -> int:
    failures = []
    config_path = REPO_ROOT / "bootstrap.toml"
    try:
        config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        print(f"FAILED: cannot parse {config_path}: {error}")
        return 1

    bootstrap = config.get("bootstrap", {})
    for required in (
        "default-environment",
        "kind-cluster",
        "kind-context",
        "registry-name",
        "flux-namespace",
    ):
        if not bootstrap.get(required):
            failures.append(f"bootstrap.{required} missing or empty")

    environments = config.get("environments", {})
    default_env = bootstrap.get("default-environment")
    if default_env and default_env not in environments:
        failures.append(f"bootstrap.default-environment '{default_env}' has no [environments.{default_env}] section")

    # kind derives the kubecontext from the cluster name; a mismatch means
    # the bootstrap targets a different cluster than the kubeconfig step.
    kind_cluster = bootstrap.get("kind-cluster")
    kind_context = bootstrap.get("kind-context")
    if kind_cluster and kind_context and kind_context != f"kind-{kind_cluster}":
        failures.append(
            f"bootstrap.kind-context '{kind_context}' is not 'kind-{kind_cluster}' for kind-cluster '{kind_cluster}'"
        )

    if "local-host" not in environments:
        failures.append("environments.local-host section missing from bootstrap.toml")

    # The imperative pins must not be reconciled from Git: a HelmRelease for
    # the same chart in the workload tree would drift against the
    # imperative install.
    flux_charts = helmrelease_charts(REPO_ROOT / "workload")
    charts = config.get("charts", {})
    if not charts:
        failures.append("[charts] is empty; the bootstrap needs its imperative pins")
    for chart, version in charts.items():
        if not re.match(r"^\d+\.\d+\.\d+$", str(version)):
            failures.append(f"charts.{chart} = '{version}' is not a plain semver pin")
        if chart in flux_charts:
            failures.append(
                f"charts.{chart} is imperative but also reconciled by {flux_charts[chart]}"
            )

    for name, env in environments.items():
        kind = env.get("kind")
        if kind != name:
            failures.append(f"environments.{name} kind {kind!r} must match its section name")
        if env.get("sync") != "oci":
            failures.append(f"environments.{name}.sync {env.get('sync')!r} must be 'oci'")
        sync_path = env.get("sync-path")
        if not sync_path or not (REPO_ROOT / sync_path).is_dir():
            failures.append(f"environments.{name}.sync-path '{sync_path}' is not a directory in the repo")
        # The kubeconfigs helper and the OCI push are environment-scoped:
        # the mise layer for the profile must exist for the environment.
        mise_layer = REPO_ROOT / f"mise.{name}.toml"
        if not mise_layer.is_file():
            failures.append(f"environments.{name} has no mise.{name}.toml environment layer")

    if failures:
        print("bootstrap.toml cross-check FAILED:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        f"bootstrap.toml cross-check OK ({len(charts)} charts, "
        f"{len(environments)} environments)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
