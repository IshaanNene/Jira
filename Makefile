SHELL := /bin/bash
.DEFAULT_GOAL := help

.PHONY: help dev stop infra build test test-rust test-web lint fmt migrate seed logs clean web-install gen-api

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

dev: ## Start infrastructure, migrate, run all services and the web app
	@./scripts/development/start.sh

stop: ## Stop services and the web app (containers keep running)
	@./scripts/development/stop.sh

infra: ## Start only the infrastructure containers
	docker compose up -d --wait postgres redis object-storage kafka opensearch mailpit gateway
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

clean: stop ## Stop everything and delete containers, volumes and build output
	docker compose down -v
	rm -rf .run
	cargo clean
