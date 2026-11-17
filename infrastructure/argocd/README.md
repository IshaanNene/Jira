# Argo CD

GitOps for staging: Argo CD keeps the cluster equal to `main`.

- `platform.yaml`: the cluster add-ons the chart relies on, as Argo CD Applications pinned to
  chart versions: External Secrets Operator (with a `ClusterSecretStore` for AWS Secrets
  Manager, reached through EKS Pod Identity, see `terraform/environments/staging`), KEDA,
  Argo Rollouts (the controller for the chart's canaries), Kyverno (admission policies,
  for the chart's image signature check) and Prometheus (chart 26.0.1, Prometheus 3.0.1, in
  `observability`). Prometheus scrapes the `meridian` namespace by pod annotations as jobs
  `services` and `gateway`, labels samples with the pod name, and loads the same alert and SLO
  rules as local Compose: `prometheus/values.yaml` is generated from
  `infrastructure/observability/prometheus` by `make prometheus-values`, and CI fails when it
  is stale, then renders it with the pinned chart and checks it with promtool.
- `staging.yaml`: an `AppProject` that may deploy only this repository's chart into the
  `meridian` namespace, and the `meridian-staging` Application (chart
  `infrastructure/helm/meridian` with `values-staging.yaml`), synced automatically with pruning
  and self-healing. Helm hooks become sync phases: the ExternalSecrets, then the migration Jobs,
  run in PreSync, before any Deployment changes.

Bootstrap, once per cluster, after installing Argo CD itself:

    kubectl apply -n argocd -f infrastructure/argocd/platform.yaml
    kubectl apply -n argocd -f infrastructure/argocd/staging.yaml

Not done: these manifests have been rendered and checked, never applied (no staging cluster
exists yet). A new build reaches staging by committing its tag to `values-staging.yaml`; each
service then rolls out as a canary (25, 50, 75 % of replicas, five minutes each) while Argo
Rollouts checks the new pods' own 5xx and slow-request ratios in Prometheus, and scales them
back if either check misses twice. Pods start only from images CI signed and attested. The
canary checks read that Prometheus (`prometheus-server.observability`). Alertmanager sends
pages to PagerDuty and Slack and tickets to Slack only (a page silences the matching ticket);
the Slack webhook URL and PagerDuty routing key are files from the `alertmanager-receivers`
Secret, which `secret-store/observability.yaml` fills from Secrets Manager through a store
usable only from `observability` and a role that can read that one entry. Write the entry
(`slack-url`, `pagerduty-routing-key`) and put `terraform output alerting_secrets_role_arn`
into that file before the first sync.
