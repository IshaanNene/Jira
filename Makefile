SHELL := /bin/bash
.DEFAULT_GOAL := help

# macOS: identity-service links xmlsec1 (SAML). Homebrew's xmlsec1 uses Homebrew's libxml2 and
# libxslt, so the build must too (two libxml2 copies in one process crash), and openssl-sys
# needs OpenSSL 3. `brew install libxmlsec1 libxslt openssl@3 pkg-config`. Exported for the
# scripts that `make dev` runs as well.
ifeq ($(shell uname),Darwin)
BREW := $(shell brew --prefix 2>/dev/null)
ifneq ($(BREW),)
export PKG_CONFIG_PATH := $(BREW)/opt/libxml2/lib/pkgconfig:$(BREW)/opt/libxslt/lib/pkgconfig:$(PKG_CONFIG_PATH)
export OPENSSL_DIR ?= $(BREW)/opt/openssl@3
export OPENSSL_ROOT_DIR ?= $(BREW)/opt/openssl@3
endif
endif

.PHONY: help dev stop infra build test test-rust test-web lint fmt migrate seed logs clean web-install gen-api observability images kind-up kind-down load resilience

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev: ## Start infrastructure, migrate, run all services and the web app
	@./scripts/development/start.sh

stop: ## Stop services and the web app (containers keep running)
	@./scripts/development/stop.sh

infra: ## Start only the infrastructure containers
	docker compose up -d --wait postgres redis object-storage kafka opensearch clickhouse mailpit gateway
	docker compose up -d object-storage-setup
	docker compose run --rm kafka-setup > /dev/null

build: ## Build all Rust services and the web app
	cargo build --workspace
	@if [ -d apps/web/node_modules ]; then pnpm --dir apps/web build; else echo "web: run 'make web-install' first"; fi

test: test-rust test-web ## Run every test suite

test-rust: infra ## Rust unit + integration tests (needs PostgreSQL from `make infra`)
	cargo test --workspace

test-web: ## Web unit tests
	@if [ -d apps/web/node_modules ]; then pnpm --dir apps/web test; else echo "web: run 'make web-install' first"; fi

lint: ## rustfmt check, clippy (warnings are errors), web lint + typecheck
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	@if [ -d apps/web/node_modules ]; then pnpm --dir apps/web lint && pnpm --dir apps/web typecheck; fi

fmt: ## Format Rust code
	cargo fmt --all

migrate: infra ## Apply all database migrations
	@./scripts/development/migrate.sh

seed: ## Load demo data through the public API (services must be running)
	@./scripts/seed/seed.sh

logs: ## Tail service logs
	@tail -n 50 -F .run/logs/*.log

web-install: ## Install web dependencies
	pnpm --dir apps/web install

gen-api: ## Regenerate TypeScript API types from the running services' OpenAPI documents
	pnpm --dir apps/web gen:api

observability: ## Start Prometheus (:59090), Grafana (:53000) and Jaeger (:56686)
	docker compose --profile observability up -d prometheus grafana jaeger

images: ## Build every service image and the web image (tag: dev)
	@./scripts/images/build.sh dev

kind-up: images ## Run the platform in a local Kubernetes cluster with the Helm chart (http://localhost:8088)
	@./scripts/kind/up.sh

kind-down: ## Delete the local Kubernetes cluster
	@./scripts/kind/down.sh

load: ## Load test through the gateway with k6 (services and seed data must be running)
	@./tests/load/run.sh

resilience: ## Stop and restart data stores and check the documented failure behaviour
	@./tests/resilience/run.sh

clean: stop ## Stop everything and delete containers, volumes and build output
	docker compose --profile observability down -v
	rm -rf .run
	cargo clean
