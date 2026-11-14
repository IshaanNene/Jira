# VPC across three availability zones: public subnets for load balancers and NAT, private
# subnets for nodes and data stores. One NAT gateway per zone, so losing a zone does not cut
# the others off.
variable "name" { type = string }
variable "cidr" { type = string }
variable "zones" { type = list(string) }

resource "aws_vpc" "this" {
  cidr_block           = var.cidr
  enable_dns_support   = true
  enable_dns_hostnames = true
  tags                 = { Name = var.name }
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = { Name = var.name }
}

resource "aws_subnet" "public" {
  count                   = length(var.zones)
  vpc_id                  = aws_vpc.this.id
  availability_zone       = var.zones[count.index]
  cidr_block              = cidrsubnet(var.cidr, 4, count.index)
  map_public_ip_on_launch = false
  tags                    = { Name = "${var.name}-public-${var.zones[count.index]}", "kubernetes.io/role/elb" = "1" }
}

resource "aws_subnet" "private" {
  count             = length(var.zones)
  vpc_id            = aws_vpc.this.id
  availability_zone = var.zones[count.index]
  cidr_block        = cidrsubnet(var.cidr, 4, count.index + 8)
  tags              = { Name = "${var.name}-private-${var.zones[count.index]}", "kubernetes.io/role/internal-elb" = "1" }
}

resource "aws_eip" "nat" {
  count  = length(var.zones)
  domain = "vpc"
}

resource "aws_nat_gateway" "this" {
  count         = length(var.zones)
  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id
  tags          = { Name = "${var.name}-${var.zones[count.index]}" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }
}

resource "aws_route_table_association" "public" {
  count          = length(var.zones)
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_route_table" "private" {
  count  = length(var.zones)
  vpc_id = aws_vpc.this.id
  route {
    cidr_block     = "0.0.0.0/0"
    nat_gateway_id = aws_nat_gateway.this[count.index].id
  }
}

resource "aws_route_table_association" "private" {
  count          = length(var.zones)
  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[count.index].id
}

output "vpc_id" { value = aws_vpc.this.id }
output "cidr" { value = aws_vpc.this.cidr_block }
output "private_subnet_ids" { value = aws_subnet.private[*].id }
output "public_subnet_ids" { value = aws_subnet.public[*].id }
