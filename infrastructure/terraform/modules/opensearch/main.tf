# OpenSearch: three data nodes over three zones inside the VPC, encrypted at rest and node to
# node, HTTPS only, fine-grained access control with an internal user for search-service.
variable "name" { type = string }
variable "subnet_ids" { type = list(string) }
variable "vpc_id" { type = string }
variable "client_security_group_ids" { type = list(string) }
variable "kms_key_arn" { type = string }
variable "instance_type" { type = string }
variable "volume_size" { type = number }

resource "aws_security_group" "this" {
  name   = "${var.name}-opensearch"
  vpc_id = var.vpc_id
  ingress {
    from_port       = 443
    to_port         = 443
    protocol        = "tcp"
    security_groups = var.client_security_group_ids
  }
}

resource "random_password" "master" {
  length           = 32
  special          = true
  override_special = "!#%*-_"
  min_special      = 1
  min_upper        = 1
  min_lower        = 1
  min_numeric      = 1
}

resource "aws_secretsmanager_secret" "master" {
  name       = "${var.name}/opensearch"
  kms_key_id = var.kms_key_arn
}

resource "aws_opensearch_domain" "this" {
  domain_name    = var.name
  engine_version = "OpenSearch_2.17"

  cluster_config {
    instance_type          = var.instance_type
    instance_count         = 3
    zone_awareness_enabled = true
    zone_awareness_config { availability_zone_count = 3 }
  }

  vpc_options {
    subnet_ids         = var.subnet_ids
    security_group_ids = [aws_security_group.this.id]
  }

  ebs_options {
    ebs_enabled = true
    volume_type = "gp3"
    volume_size = var.volume_size
  }

  encrypt_at_rest {
    enabled    = true
    kms_key_id = var.kms_key_arn
  }
  node_to_node_encryption { enabled = true }
  domain_endpoint_options {
    enforce_https       = true
    tls_security_policy = "Policy-Min-TLS-1-2-2019-07"
  }

  advanced_security_options {
    enabled                        = true
    internal_user_database_enabled = true
    master_user_options {
      master_user_name     = "platform"
      master_user_password = random_password.master.result
    }
  }
}

resource "aws_secretsmanager_secret_version" "master" {
  secret_id = aws_secretsmanager_secret.master.id
  secret_string = jsonencode({
    OPENSEARCH_URL = "https://platform:${urlencode(random_password.master.result)}@${aws_opensearch_domain.this.endpoint}"
  })
}

output "secret_arn" { value = aws_secretsmanager_secret.master.arn }
