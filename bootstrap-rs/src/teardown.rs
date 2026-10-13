//! Teardown: the port of `teardown.sh` (issue #14, local-only).
//!
//! Destroys everything the bootstrap creates, in reverse creation order:
//! the exported host kubeconfig, the local registry container, and the
//! kind cluster itself. There is no CAPI, no pivot, and no management
//! cluster to unwind, so the run is three steps and every step is
//! idempotent: a resource that is already absent is skipped, and a run
//! that was interrupted can be resumed by rerunning.

use anyhow::{Context, Result};

use crate::engine;
use crate::{capture_lossy, run, Config};

pub async fn run_teardown(cfg: &Config) -> Result<()> {
    let kind_cluster = &cfg.repo.bootstrap.kind_cluster;
    let registry_name = &cfg.repo.bootstrap.registry_name;

    println!(
        ">>> Teardown of '{profile}': registry '{registry_name}', kind cluster '{kind_cluster}'",
        profile = cfg.profile
    );

    // Step 1: remove the exported host kubeconfig. Independent of the
    // cluster and safe either way, so it goes first.
    remove_kubeconfig(cfg).await?;

    // Engine resolution gates the container-level steps: without a running
    // daemon neither the registry container nor the kind cluster can be
    // reached, so a teardown that cannot verify its own success must not
    // claim to be complete.
    let resolved = engine::resolve(
        cfg.container_engine.clone(),
        cfg.engine_sock.clone(),
        cfg.toolbox,
    )
    .await
    .context("teardown needs a running container engine (docker or podman) to remove the registry container and the kind cluster")?;

    // Step 2: remove the local registry container. It is attached to the
    // shared 'kind' docker network, so remove it before the cluster goes;
    // a dangling container on a destroyed network needs a manual cleanup.
    remove_registry_container(&resolved.engine, registry_name).await?;

    // Step 3: delete the kind cluster (control plane + node containers).
    // The cluster is addressed by explicit name, so a stale kubectl
    // current-context cannot redirect the destructive operation.
    delete_kind_cluster(kind_cluster).await?;

    println!();
    println!(">>> Teardown complete: the bootstrap left nothing behind.");
    Ok(())
}

/// Remove the exported host kubeconfig file. Idempotent: a missing file is
/// a clean no-op (the bootstrap may never have finished writing it).
async fn remove_kubeconfig(cfg: &Config) -> Result<()> {
    let kc = &cfg.kubeconfig;
    match std::fs::remove_file(kc) {
        Ok(()) => println!("    Removed kubeconfig {}", kc.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("    Kubeconfig {} not present; skipping", kc.display())
        }
        Err(e) => {
            anyhow::bail!("failed to remove kubeconfig {}: {e}", kc.display())
        }
    }
    Ok(())
}

/// Remove the local registry container by exact name. Idempotent: a
/// missing container is a clean no-op; a present container that fails to
/// remove is a real error.
async fn remove_registry_container(engine: &str, registry_name: &str) -> Result<()> {
    let exists = capture_lossy(
        engine,
        &[
            "ps",
            "-a",
            "--filter",
            &format!("name=^{registry_name}$"),
            "--format",
            "{{.Names}}",
        ],
    )
    .await
    .lines()
    .any(|l| l.trim() == registry_name);
    if !exists {
        println!("    Registry container '{registry_name}' not present; skipping");
        return Ok(());
    }
    println!("    Removing registry container '{registry_name}'...");
    run(engine, &["rm", "-f", registry_name]).await
}

/// Delete the kind cluster by explicit name. Idempotent: a missing cluster
/// is a clean no-op (rerun-safe teardown).
async fn delete_kind_cluster(kind_cluster: &str) -> Result<()> {
    let clusters = capture_lossy("kind", &["get", "clusters"]).await;
    if !clusters.lines().any(|l| l.trim() == kind_cluster) {
        println!("    Kind cluster '{kind_cluster}' not present; skipping");
        return Ok(());
    }
    println!("    Deleting kind cluster '{kind_cluster}'...");
    run("kind", &["delete", "cluster", "--name", kind_cluster]).await
}
