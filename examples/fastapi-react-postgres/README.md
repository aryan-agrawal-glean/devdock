# Example: FastAPI + React + PostgreSQL

**Reference repo:** [tiangolo/full-stack-fastapi-template](https://github.com/tiangolo/full-stack-fastapi-template)
**Stack:** Python 3.11 FastAPI backend · React + TypeScript (Vite) frontend · PostgreSQL 16

---

## What This Demonstrates

- Two-service Docker setup (backend API + frontend dev server)
- Non-Bazel builds: both services use `entrypoint.sh` for startup
- Custom infra: PostgreSQL 16 overrides devdock's default SQL infra
- Source code live-mounting via `extra_mounts` (no rebuild on code changes)
- Cross-service dependency: frontend waits for backend to be healthy
- Profile variants: `default` (all services) vs `api-only` (backend + db only)

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Docker | 24+ | docker.com |
| Tilt | 0.33+ | `curl -fsSL https://raw.githubusercontent.com/tilt-dev/tilt/master/scripts/install.sh \| bash` |
| DevDock | latest | `cargo install devdock-cli` |

---

## Repo Layout (tiangolo/full-stack-fastapi-template)

```
full-stack-fastapi-template/
├── backend/               ← FastAPI app (Python, pyproject.toml)
│   ├── app/
│   │   ├── main.py
│   │   ├── api/
│   │   └── alembic/       ← DB migrations
│   └── pyproject.toml
├── frontend/              ← React + TypeScript (Vite)
│   ├── src/
│   └── package.json
└── compose.yml            ← original Docker Compose (replaced by DevDock)
```

---

## Adding DevDock to This Repo

Copy these files into the repo root:

```
full-stack-fastapi-template/
├── .devdock/
│   ├── devdock.toml
│   └── profiles/
│       ├── default.yaml       ← backend + frontend + postgres
│       └── api-only.yaml      ← backend + postgres only
└── devdock/
    ├── infra/
    │   └── sql/
    │       └── docker-compose.yaml   ← PostgreSQL 16 override
    └── services/
        ├── backend/
        │   ├── devdock_config.json
        │   └── entrypoint.sh         ← pip install + migrations + fastapi
        └── frontend/
            ├── devdock_config.json
            └── entrypoint.sh         ← npm install + vite dev server
```

---

## Running

```bash
# Clone the reference repo
git clone https://github.com/tiangolo/full-stack-fastapi-template myapp
cd myapp

# Copy devdock config (from this directory)
cp -r /path/to/devdock/examples/fastapi-react-postgres/.devdock .
cp -r /path/to/devdock/examples/fastapi-react-postgres/devdock .

# Start everything (backend + frontend + postgres)
devdock start default

# Start just the API and database
devdock start api-only

# Open Tilt dashboard
open http://localhost:10350
```

---

## Expected Results

After `devdock start default`, the Tilt dashboard at http://localhost:10350 shows:

| Resource | Status | URL |
|----------|--------|-----|
| `sql` (postgres) | `runtime=ok` | postgres://localhost:5432 |
| `backend` | `runtime=ok` | http://localhost:8000 |
| `frontend` | `runtime=ok` | http://localhost:5173 |

Verification commands:
```bash
# Backend health check
curl http://localhost:8000/api/v1/utils/health-check/
# → true

# Login (admin credentials set via env vars)
curl -X POST http://localhost:8000/api/v1/login/access-token \
  -d "username=admin@example.com&password=changeme"
# → {"access_token": "...", "token_type": "bearer"}

# Interactive API docs
open http://localhost:8000/docs

# Frontend dev server (Vite HMR enabled)
open http://localhost:5173
```

> **Verified** ✅: All services start successfully. Backend serves JWT auth; frontend hot-reloads from source.

---

## Config Decisions

- **`extra_mounts`** — `backend/` and `frontend/` source dirs are bind-mounted into the container. Code changes are reflected without rebuilding the Docker image.
- **`entrypoint.sh`** — Both services use shell scripts for startup. Backend runs `pip install -e .` (installs from `pyproject.toml`), runs Alembic migrations, and starts FastAPI. Frontend runs `npm install` and starts Vite dev server.
- **PostgreSQL override** — `devdock/infra/sql/docker-compose.yaml` replaces devdock's default MySQL infra with PostgreSQL 16.
- **`PYTHONPATH=/app`** — Required so Python can find the `app` package (at `/app/app/`) when running migrations and startup scripts.
- **`.devdock.local` DNS** — DevDock puts all containers on a shared `devdock_network`. `sql.devdock.local` resolves to the PostgreSQL container from within the backend container.

---

## Notes

- The original repo uses `compose.yml` with Traefik for HTTPS in production. DevDock replaces this for local development — no Traefik needed.
- Alembic migrations run automatically on every backend container start via `entrypoint.sh`.
- The `pip install -e .` in the backend entrypoint installs all dependencies from `pyproject.toml` at runtime, ensuring the package is importable as `app`.
- If you add Celery workers, create `devdock/services/worker/devdock_config.json` and add `worker: { type: local }` to the profile.
