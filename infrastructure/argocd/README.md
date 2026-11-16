# Argo CD

GitOps for staging: Argo CD keeps the cluster equal to `main`.

- `platform.yaml`: the cluster add-ons the chart relies on, as Argo CD Applications pinned to
  chart versions: External Secrets Operator (with a `ClusterSecretStore` for AWS Secrets
  Manager, reached through EKS Pod Identity, see `terraform/environments/staging`) and KEDA.
- `staging.yaml`: an `AppProject` that may deploy only this repository's chart into the
  `meridian` namespace, and the `meridian-staging` Application (chart
  `infrastructure/helm/meridian` with `values-staging.yaml`), synced automatically with pruning
  and self-healing. Helm hooks become sync phases: the ExternalSecrets, then the migration Jobs,
  run in PreSync, before any Deployment changes.

Bootstrap, once per cluster, after installing Argo CD itself:

    kubectl apply -n argocd -f infrastructure/argocd/platform.yaml
    kubectl apply -n argocd -f infrastructure/argocd/staging.yaml

Not done: these manifests have been rendered and checked, never applied (no staging cluster
exists yet). No Argo Rollouts canaries; a new build reaches staging by committing its tag to
`values-staging.yaml`.
