# Example: Django + Celery Worker

**Pattern:** Django REST API + Celery async worker + Redis (broker) + PostgreSQL
**Reference:** Standard Django+Celery project structure (e.g., [testdrivenio/django-celery](https://github.com/testdrivenio/django-celery))
**Stack:** Python 3.11 · Django 4.2 · Celery 5 · Redis 7 · PostgreSQL 15

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

## Typical Repo Layout

```
my-django-app/
├── myapp/
│   ├── settings/
│   │   ├── base.py
│   │   └── local.py
│   ├── celery.py          ← Celery app init
│   └── wsgi.py
├── tasks/                 ← Celery task modules
├── api/                   ← Django REST endpoints
├── Dockerfile
├── requirements.txt
└── manage.py
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
# Django health endpoint (if you have one)
curl http://localhost:8000/health/

# Admin panel
open http://localhost:8000/admin/

# Trigger a Celery task and watch the worker container logs in Tilt
curl -X POST http://localhost:8000/api/tasks/ -d '{"task": "my_task"}'
# → Tilt shows the worker container logging the task execution
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
