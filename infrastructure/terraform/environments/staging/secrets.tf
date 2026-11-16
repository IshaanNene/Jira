# Where the chart's Secrets come from (infrastructure/helm/meridian, externalSecrets): one
# Secrets Manager entry per Kubernetes Secret, named <name>/<Secret name>, each a JSON object of
# the keys listed in values.yaml. Terraform makes the entries, encrypted with the data key; the
# values are written out of band (per-service database roles and passwords, the internal
# service secret), so none of them is ever in Terraform state.
locals {
  # Services with a PostgreSQL database: their runtime and migration credentials.
  database_services = ["tenant", "identity", "user", "permission", "project", "workflow", "issue", "agile", "notification", "search", "analytics"]
  app_secrets = concat(
    ["meridian-shared"],
    [for s in local.database_services : "meridian-${s}"],
    [for s in local.database_services : "meridian-${s}-migrate"],
  )
}

resource "aws_secretsmanager_secret" "app" {
  for_each                = toset(local.app_secrets)
  name                    = "${var.name}/${each.key}"
  kms_key_id              = aws_kms_key.data.arn
  recovery_window_in_days = 7
}

# The External Secrets Operator reads those entries, and nothing else, through EKS Pod
# Identity: its service account (namespace external-secrets) is the only one given this role.
resource "aws_eks_addon" "pod_identity" {
  cluster_name = module.cluster.name
  addon_name   = "eks-pod-identity-agent"
}

data "aws_iam_policy_document" "pod_identity_assume" {
  statement {
    actions = ["sts:AssumeRole", "sts:TagSession"]
    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "external_secrets" {
  name               = "${var.name}-external-secrets"
  assume_role_policy = data.aws_iam_policy_document.pod_identity_assume.json
}

data "aws_iam_policy_document" "external_secrets" {
  statement {
    actions   = ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]
    resources = [for s in aws_secretsmanager_secret.app : s.arn]
  }
  statement {
    actions   = ["kms:Decrypt"]
    resources = [aws_kms_key.data.arn]
    condition {
      test     = "StringEquals"
      variable = "kms:ViaService"
      values   = ["secretsmanager.${var.region}.amazonaws.com"]
    }
  }
}

resource "aws_iam_role_policy" "external_secrets" {
  name   = "read-meridian-secrets"
  role   = aws_iam_role.external_secrets.id
  policy = data.aws_iam_policy_document.external_secrets.json
}

resource "aws_eks_pod_identity_association" "external_secrets" {
  cluster_name    = module.cluster.name
  namespace       = "external-secrets"
  service_account = "external-secrets"
  role_arn        = aws_iam_role.external_secrets.arn
  depends_on      = [aws_eks_addon.pod_identity]
}

output "app_secret_names" { value = [for s in aws_secretsmanager_secret.app : s.name] }
