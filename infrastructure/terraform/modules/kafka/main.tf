# MSK: three brokers in three zones, TLS in transit, SASL/SCRAM for clients (the services'
# KAFKA_SECURITY_PROTOCOL=sasl_ssl), replication factor 3 with min.insync.replicas 2.
variable "name" { type = string }
variable "subnet_ids" { type = list(string) }
variable "vpc_id" { type = string }
variable "client_security_group_ids" { type = list(string) }
variable "kms_key_arn" { type = string }
variable "instance_type" { type = string }
variable "volume_size" { type = number }

resource "aws_security_group" "this" {
  name   = "${var.name}-kafka"
  vpc_id = var.vpc_id
  ingress {
    from_port       = 9096
    to_port         = 9096
    protocol        = "tcp"
    security_groups = var.client_security_group_ids
  }
}

resource "aws_msk_configuration" "this" {
  name              = var.name
  kafka_versions    = ["3.8.x"]
  server_properties = <<-PROPS
    auto.create.topics.enable=false
    default.replication.factor=3
    min.insync.replicas=2
    num.partitions=12
    unclean.leader.election.enable=false
  PROPS
}

resource "aws_msk_cluster" "this" {
  cluster_name           = var.name
  kafka_version          = "3.8.x"
  number_of_broker_nodes = length(var.subnet_ids)

  broker_node_group_info {
    instance_type   = var.instance_type
    client_subnets  = var.subnet_ids
    security_groups = [aws_security_group.this.id]
    storage_info {
      ebs_storage_info { volume_size = var.volume_size }
    }
  }

  configuration_info {
    arn      = aws_msk_configuration.this.arn
    revision = aws_msk_configuration.this.latest_revision
  }

  encryption_info {
    encryption_at_rest_kms_key_arn = var.kms_key_arn
    encryption_in_transit {
      client_broker = "TLS"
      in_cluster    = true
    }
  }

  client_authentication {
    sasl { scram = true }
    unauthenticated = false
  }
}

# MSK reads SCRAM credentials from Secrets Manager secrets whose names start with AmazonMSK_.
resource "random_password" "scram" {
  length  = 32
  special = false
}

resource "aws_secretsmanager_secret" "scram" {
  name       = "AmazonMSK_${var.name}_platform"
  kms_key_id = var.kms_key_arn
}

resource "aws_secretsmanager_secret_version" "scram" {
  secret_id     = aws_secretsmanager_secret.scram.id
  secret_string = jsonencode({ username = "platform", password = random_password.scram.result })
}

resource "aws_msk_scram_secret_association" "this" {
  cluster_arn     = aws_msk_cluster.this.arn
  secret_arn_list = [aws_secretsmanager_secret.scram.arn]
  depends_on      = [aws_secretsmanager_secret_version.scram]
}

output "bootstrap_brokers" { value = aws_msk_cluster.this.bootstrap_brokers_sasl_scram }
output "credentials_secret_arn" { value = aws_secretsmanager_secret.scram.arn }
