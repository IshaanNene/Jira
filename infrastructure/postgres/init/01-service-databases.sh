#!/usr/bin/env bash
# Development-only bootstrap: one database and two roles per service.
#   <svc>_owner  runs migrations and owns the tables
#   <svc>_app    runtime role: DML only, never bypasses row-level security
# Passwords equal role names here on purpose — this file is for local containers only.
# Production roles and credentials are provisioned by Terraform + External Secrets.
set -euo pipefail

SERVICES="identity tenant user project issue workflow agile permission search notification automation integration analytics"

for svc in $SERVICES; do
  db="${svc}_db"; owner="${svc}_owner"; app="${svc}_app"
  psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres <<SQL
CREATE ROLE ${owner} LOGIN PASSWORD '${owner}' NOSUPERUSER NOBYPASSRLS;
CREATE ROLE ${app}   LOGIN PASSWORD '${app}'   NOSUPERUSER NOBYPASSRLS;
CREATE DATABASE ${db} OWNER ${owner};
REVOKE ALL ON DATABASE ${db} FROM PUBLIC;
GRANT CONNECT ON DATABASE ${db} TO ${app};
SQL
  psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$db" <<SQL
CREATE EXTENSION IF NOT EXISTS citext;
GRANT USAGE ON SCHEMA public TO ${app};
ALTER DEFAULT PRIVILEGES FOR ROLE ${owner} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${app};
ALTER DEFAULT PRIVILEGES FOR ROLE ${owner} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${app};
SQL
done
