# Example: Django + Celery Worker

**Pattern:** Django REST API + Celery async worker + Redis (broker) + PostgreSQL
**Reference:** [saasitive/docker-compose-django-celery-redis-postgres](https://github.com/saasitive/docker-compose-django-celery-redis-postgres)
**Stack:** Python 3.8 · Django 4.1 · Celery 5 · Redis 7 · PostgreSQL 16

> **Verified:** All services (`sql`, `redis`, `api`, `worker`) reach `runtime=ok`. Django admin accessible at http://localhost:8000/admin/ (HTTP 302). Celery worker connects to Redis and reports `celery@<id> ready.`

---

## What This Demonstrates

- **Worker pattern**: separate `api` and `worker` services sharing the same codebase
- `worker` depends on `api` being healthy before starting (via `dependencies`)
- Redis as both the Celery broker and Django cache (shared infra)
- PostgreSQL as the primary database
- Profile variants: `default` (all) vs `api-only` (no background worker)

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Docker | 24+ | docker.com |
| Tilt | 0.33+ | tilt.dev |
| DevDock | latest | `cargo install devdock-cli` |

---

## Repo Layout (saasitive reference)

```
docker-compose-django-celery-redis-postgres/
├── requirements.txt       ← pip deps (django, celery[redis], psycopg2-binary, etc.)
├── backend/               ← Django project root (contains manage.py)
│   ├── manage.py
│   └── backend/           ← Django app (settings.py, celery.py, urls.py)
│       ├── settings.py    ← reads DJANGO_DB, POSTGRES_*, BROKER_URL from env
│       └── celery.py      ← Celery app: `celery -A backend worker`
├── assignments/           ← Example Celery task module
└── docker/                ← Original Docker configs (not used by devdock)
```

---

## Adding DevDock to This Repo

```
my-django-app/
├── .devdock/
│   ├── devdock.toml
│   └── profiles/
│       ├── default.yaml      ← api + worker + sql + redis
│       └── api-only.yaml     ← api + sql + redis (no worker)
└── devdock/
    └── services/
        ├── api/
        │   └── devdock_config.json
        └── worker/
            └── devdock_config.json
```

---

## Running

```bash
# Start everything
devdock start default

# Start without the Celery worker (e.g., while debugging tasks)
devdock start api-only

open http://localhost:10350
```

---

## Expected Results

### `devdock start default`

| Resource | Status | URL |
|----------|--------|-----|
| `sql` (postgres) | `runtime=ok` | localhost:5432 |
| `redis` | `runtime=ok` | localhost:6379 |
| `api` | `runtime=ok` | http://localhost:8000 |
| `worker` | `runtime=ok` | (no HTTP port — Celery process) |

Verify:
```bash
# Django admin panel — expect HTTP 302 (redirect to login)
curl -o /dev/null -w "%{http_code}" http://localhost:8000/admin/
# → 302

# Check Celery worker connected (in Tilt logs for 'worker')
# → celery@<id> ready.
docker logs devdock-worker 2>&1 | grep "ready"
```

---

## Config Decisions

- **`worker` has no `port`** — Celery workers don't expose HTTP. DevDock still manages the container lifecycle.
- **`worker` depends on `api`** — ensures Django migrations and setup complete before workers start. Without this, workers might fail to connect to the database on a cold start.
- **Redis on `redis.devdock.local:6379`** — devdock's network gives all containers `.devdock.local` DNS names. Use this in `CELERY_BROKER_URL` and `DJANGO_CACHE_URL`.
- **`DJANGO_SETTINGS_MODULE = myapp.settings.local`** — points Django at the local settings file that reads env vars for database and Redis URLs.

---

## Adapting to Your Project

1. **Different settings module**: change `DJANGO_SETTINGS_MODULE` to match your project
2. **Multiple workers**: add `worker-priority` or `worker-email` as separate services with different `CELERY_QUEUES` env vars
3. **Celery Beat** (scheduler): add a `beat` service with the same config but command override: `["celery", "-A", "myapp", "beat", "--loglevel=info"]`
4. **Flower** (Celery monitoring UI): add a `flower` service on port 5555

---

## Notes

- Run migrations before the API starts: add a `entrypoint` script or a devdock `before_tilt` plugin hook that runs `python manage.py migrate`.
- Static files: run `python manage.py collectstatic` as part of the build step or as a devdock hook.
- If you use `dj-database-url`, set `DATABASE_URL` to `postgresql://postgres:postgres@sql.devdock.local:5432/myapp`.
