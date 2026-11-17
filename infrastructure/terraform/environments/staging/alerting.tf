# Where Alertmanager's notifications go: one Secrets Manager entry holding the Slack incoming
# webhook URL (`slack-url`) and the PagerDuty Events v2 routing key (`pagerduty-routing-key`),
# written out of band like every other secret value.
#
# The observability namespace reads it through its own ClusterSecretStore
# (infrastructure/argocd/secret-store/observability.yaml), which assumes the role below. That
# role may read this entry only, so nothing running in observability can ask the operator for
# the application's database credentials.
resource "aws_secretsmanager_secret" "alerting" {
  name                    = "${var.name}/alertmanager-receivers"
  kms_key_id              = aws_kms_key.data.arn
  recovery_window_in_days = 7
}

data "aws_iam_policy_document" "alerting_assume" {
  statement {
    actions = ["sts:AssumeRole", "sts:TagSession"]
    principals {
      type        = "AWS"
      identifiers = [aws_iam_role.external_secrets.arn]
    }
  }
}

resource "aws_iam_role" "alerting_secrets" {
  name               = "${var.name}-alerting-secrets"
  assume_role_policy = data.aws_iam_policy_document.alerting_assume.json
}

data "aws_iam_policy_document" "alerting_secrets" {
  statement {
    actions   = ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]
    resources = [aws_secretsmanager_secret.alerting.arn]
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

resource "aws_iam_role_policy" "alerting_secrets" {
  name   = "read-alerting-secret"
  role   = aws_iam_role.alerting_secrets.id
  policy = data.aws_iam_policy_document.alerting_secrets.json
}

# The operator may become that role (and only for the observability store, which names it).
data "aws_iam_policy_document" "external_secrets_assume_alerting" {
  statement {
    actions   = ["sts:AssumeRole", "sts:TagSession"]
    resources = [aws_iam_role.alerting_secrets.arn]
  }
}

resource "aws_iam_role_policy" "external_secrets_assume_alerting" {
  name   = "assume-alerting-secrets"
  role   = aws_iam_role.external_secrets.id
  policy = data.aws_iam_policy_document.external_secrets_assume_alerting.json
}

output "alerting_secrets_role_arn" { value = aws_iam_role.alerting_secrets.arn }
