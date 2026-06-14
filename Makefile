# TakoIA core-backend — developer tasks.
# Hot reload: run `make dev` (backend auto-reload + frontend HMR together),
# or run `make dev-api` and `make dev-web` in two terminals.

.PHONY: help demo docker setup dev dev-api dev-web build build-frontend run test fmt clippy db-reset

help:
	@echo "TakoIA core-backend"
	@echo "  make demo           one command: setup + build + run (http://localhost:8080)"
	@echo "  make docker         build & run everything in Docker (no Rust/Bun needed)"
	@echo "  make setup          install frontend deps + create .env"
	@echo "  make dev            run backend (auto-reload) + frontend (HMR)"
	@echo "  make dev-api        backend only, auto-reload on Rust changes"
	@echo "  make dev-web        frontend only, Vite HMR on http://localhost:5173"
	@echo "  make build          production build (frontend bundle + release binary)"
	@echo "  make run            run the release binary (serves built frontend)"
	@echo "  make test           cargo test"
	@echo "  make fmt clippy     format / lint"
	@echo "  make db-reset       delete the SQLite database"

# One-shot local run: install deps, create .env, build everything, serve.
demo: setup build run

# Zero-prerequisite path (only Docker required).
docker:
	docker compose up --build

setup:
	cd frontend && bun install
	@test -f .env || ( SAMPLE=$$(test -f .env.example && echo .env.example || echo .env-sample); \
		sed 's|REPLACE_ME_WITH_openssl_rand_base64_32|'"$$(openssl rand -base64 32)"'|' $$SAMPLE > .env && \
		echo "created .env from $$SAMPLE with a fresh MASTER_KEY" )

# Run both dev servers; Ctrl-C stops both.
dev:
	@echo "backend  -> http://localhost:8080"
	@echo "frontend -> http://localhost:5173 (use this one, it proxies /api)"
	@trap 'kill 0' INT TERM EXIT; \
	$(MAKE) dev-api & \
	$(MAKE) dev-web & \
	wait

dev-api:
	cargo watch -x run

dev-web:
	cd frontend && bun run dev

build-frontend:
	cd frontend && bun run build

build: build-frontend
	cargo build --release

run:
	@test -d frontend/dist || ( echo "frontend bundle missing — building it once..." && $(MAKE) build-frontend )
	cargo run --release

test:
	cargo test

fmt:
	cargo fmt

clippy:
	cargo clippy --all-targets -- -D warnings

db-reset:
	rm -f data/takoia.db data/takoia.db-shm data/takoia.db-wal
	@echo "database removed; it will be recreated and migrated on next run"
