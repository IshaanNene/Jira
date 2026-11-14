# Staging: one region, three availability zones, managed data stores. Production is the same
# composition with larger sizes (environments/production, to be added when it is provisioned).
#
# State lives in S3 with locking; the bucket is created once by hand (bootstrap) and named in
# backend.hcl:  terraform init -backend-config=backend.hcl
terraform {
  required_version = ">= 1.10"
  required_providers {
    aws    = { source = "hashicorp/aws", version = "~> 5.80" }
    random = { source = "hashicorp/random", version = "~> 3.6" }
  }
  backend "s3" {
    key          = "meridian/staging.tfstate"
    encrypt      = true
    use_lockfile = true
  }
}

variable "region" {
  type    = string
  default = "eu-west-1"
}

variable "name" {
  type    = string
  default = "meridian-staging"
}

provider "aws" {
  region = var.region
  default_tags {
    tags = { project = "meridian", environment = "staging", managed-by = "terraform" }
  }
}

data "aws_availability_zones" "available" { state = "available" }

locals {
  zones = slice(data.aws_availability_zones.available.names, 0, 3)
}

# One customer-managed key for data at rest across the environment.
resource "aws_kms_key" "data" {
  description         = "${var.name} data at rest"
  enable_key_rotation = true
}

resource "aws_kms_alias" "data" {
  name          = "alias/${var.name}"
  target_key_id = aws_kms_key.data.key_id
}

module "network" {
  source = "../../modules/network"
  name   = var.name
  cidr   = "10.40.0.0/16"
  zones  = local.zones
}

module "cluster" {
  source              = "../../modules/cluster"
  name                = var.name
  kubernetes_version  = "1.31"
  subnet_ids          = module.network.private_subnet_ids
  kms_key_arn         = aws_kms_key.data.arn
  node_instance_types = ["m7g.large"]
  node_min            = 3
  node_max            = 9
}

module "postgres" {
  source                    = "../../modules/postgres"
  name                      = var.name
  vpc_id                    = module.network.vpc_id
  subnet_ids                = module.network.private_subnet_ids
  client_security_group_ids = [module.cluster.security_group_id]
  kms_key_arn               = aws_kms_key.data.arn
  instance_class            = "db.m7g.large"
  allocated_storage         = 100
}

module "kafka" {
  source                    = "../../modules/kafka"
  name                      = var.name
  vpc_id                    = module.network.vpc_id
  subnet_ids                = module.network.private_subnet_ids
  client_security_group_ids = [module.cluster.security_group_id]
  kms_key_arn               = aws_kms_key.data.arn
  instance_type             = "kafka.m7g.large"
  volume_size               = 200
}

module "redis" {
  source                    = "../../modules/redis"
  name                      = var.name
  vpc_id                    = module.network.vpc_id
  subnet_ids                = module.network.private_subnet_ids
  client_security_group_ids = [module.cluster.security_group_id]
  kms_key_arn               = aws_kms_key.data.arn
  node_type                 = "cache.m7g.large"
}

module "opensearch" {
  source                    = "../../modules/opensearch"
  name                      = var.name
  vpc_id                    = module.network.vpc_id
  subnet_ids                = module.network.private_subnet_ids
  client_security_group_ids = [module.cluster.security_group_id]
  kms_key_arn               = aws_kms_key.data.arn
  instance_type             = "m7g.large.search"
  volume_size               = 100
}

module "storage" {
  source      = "../../modules/storage"
  bucket      = "${var.name}-attachments"
  kms_key_arn = aws_kms_key.data.arn
}

# Inputs for the Helm values and External Secrets of this environment.
output "cluster_name" { value = module.cluster.name }
output "vpc_cidr" { value = module.network.cidr }
output "postgres_endpoint" { value = module.postgres.endpoint }
output "kafka_bootstrap_brokers" { value = module.kafka.bootstrap_brokers }
output "attachments_bucket" { value = module.storage.bucket }
output "secret_arns" {
  value = {
    postgres_admin = module.postgres.master_secret_arn
    kafka          = module.kafka.credentials_secret_arn
    redis          = module.redis.secret_arn
    opensearch     = module.opensearch.secret_arn
  }
}
