"""The API for the browser end-to-end flows (apps/web/e2e, Playwright project `api`): Postgres 16 +
pgvector and Redis 7 in testcontainers, the migrations, the sim_runner worker in this process on the
native ngspice, and the real app (`create_app`) under uvicorn, generating with `LLM_PROVIDER=fake`
from `LLM_FAKE_SCRIPT` (default: apps/api/fake/script.json). The same setup as apps/api/tests.

    .venv/Scripts/python tools/e2e/stack.py [--port 8100]
    .venv/Scripts/python tools/e2e/stack.py --port 8000 --llm-from .env   # the provider in .env (real calls)

With `--llm-from FILE` it generates with the provider that file's LLM_* settings name (variables already set
win), for trying the editor against a real model without building the compose images. The databases
live as long as the process.

Ready when `GET /healthz` answers. Stops on Ctrl+C or SIGTERM; the containers go with the process
(testcontainers' reaper removes them if it is killed).

`databases`, `sim_worker` and `api` are also what the generation evals (evals/run_evals.py) run on.
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
import os
import secrets
import signal
import socket
import sys
from collections.abc import AsyncIterator, Iterator
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


def postgres():
    from testcontainers.core.container import DockerContainer
    from testcontainers.core.wait_strategies import LogMessageWaitStrategy

    ready = LogMessageWaitStrategy("database system is ready to accept connections", times=2).with_startup_timeout(90)
    return (
        DockerContainer("pgvector/pgvector:pg16")
        .with_env("POSTGRES_USER", "tutor")
        .with_env("POSTGRES_PASSWORD", "tutor")
        .with_env("POSTGRES_DB", "tutor")
        .with_exposed_ports(5432)
        .waiting_for(ready)
    )


def redis():
    from testcontainers.core.container import DockerContainer
    from testcontainers.core.wait_strategies import LogMessageWaitStrategy

    ready = LogMessageWaitStrategy("Ready to accept connections").with_startup_timeout(60)
    return DockerContainer("redis:7-alpine").with_exposed_ports(6379).waiting_for(ready)


@contextlib.contextmanager
def databases() -> Iterator[tuple[str, str]]:
    """Postgres (migrated) and Redis in testcontainers: `(database_url, redis_url)`."""
    from tutor_api.db import migrate

    with postgres() as pg, redis() as rd:
        database_url = f"postgresql+asyncpg://tutor:tutor@{pg.get_container_host_ip()}:{pg.get_exposed_port(5432)}/tutor"
        redis_url = f"redis://{rd.get_container_host_ip()}:{rd.get_exposed_port(6379)}/0"
        migrate.wait_until_ready(database_url)
        migrate.upgrade(database_url)
        yield database_url, redis_url


@contextlib.asynccontextmanager
async def sim_worker(redis_url: str, max_jobs: int = 4) -> AsyncIterator[None]:
    """The sim_runner worker consuming `redis_url`'s queue in this event loop, on the native ngspice."""
    from arq.worker import Worker
    from sim_runner import client as sim_client
    from sim_runner import ngspice_batch
    from sim_runner import worker as w

    if ngspice_batch.ngspice_path() is None:
        sys.exit("ngspice not built: run third_party/ngspice/build-native.sh")
    sim_redis = await sim_client.connect(redis_url)
    s = w.WorkerSettings
    worker = Worker(
        functions=s.functions, queue_name=s.queue_name, redis_pool=sim_redis, max_jobs=max_jobs,
        keep_result=s.keep_result, poll_delay=s.poll_delay, job_serializer=s.job_serializer,
        job_deserializer=s.job_deserializer, on_startup=s.on_startup, handle_signals=False,
    )
    work = asyncio.create_task(worker.async_run())
    try:
        yield
    finally:
        # arq's Worker.close() signals itself with SIGUSR1, which Windows lacks: cancel its tasks.
        tasks = [work, *worker.tasks.values()]
        for t in tasks:
            t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        await sim_redis.aclose()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@contextlib.asynccontextmanager
async def api(app, port: int | None = None) -> AsyncIterator[tuple[str, asyncio.Task]]:
    """`app` under uvicorn in this event loop; yields its base URL and the task serving it."""
    import uvicorn

    port = port or free_port()
    server = uvicorn.Server(uvicorn.Config(app, host="127.0.0.1", port=port, log_level="warning", lifespan="on"))
    serving = asyncio.create_task(server.serve())
    while not server.started:
        if serving.done():
            serving.result()
        await asyncio.sleep(0.02)
    try:
        yield f"http://127.0.0.1:{port}", serving
    finally:
        server.should_exit = True
        await serving


async def serve(port: int, database_url: str, redis_url: str, llm: str) -> None:
    from tutor_api.config import Settings
    from tutor_api.llm.config import gateway_from_env
    from tutor_api.main import create_app
    from tutor_api.orchestrator import Orchestrator

    gateway = gateway_from_env()
    assert gateway is not None
    settings = Settings(database_url=database_url, redis_url=redis_url, auth_secret=secrets.token_urlsafe(32))
    app = create_app(settings, Orchestrator(gateway))
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError, ValueError):  # Windows has no loop signal handlers
            loop.add_signal_handler(sig, stop.set)
    async with sim_worker(redis_url), api(app, port) as (url, serving):
        print(f"e2e API on {url} ({llm})", flush=True)
        await asyncio.wait([serving, asyncio.create_task(stop.wait())], return_when=asyncio.FIRST_COMPLETED)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--port", type=int, default=int(os.environ.get("E2E_API_PORT") or 8100))
    ap.add_argument("--llm-from", type=Path, metavar="ENV_FILE",
                    help="generate with the provider this file's LLM_* settings name, not the fake script")
    args = ap.parse_args()
    if args.llm_from:
        for line in args.llm_from.read_text(encoding="utf-8").splitlines():
            key, sep, value = line.strip().partition("=")
            if sep and not key.startswith("#"):
                os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))
        llm = f"LLM_PROVIDER={os.environ.get('LLM_PROVIDER', '')}, from {args.llm_from}"
    else:
        os.environ["LLM_PROVIDER"] = "fake"
        os.environ.setdefault("LLM_FAKE_SCRIPT", str(REPO / "apps/api/fake/script.json"))
        llm = f"LLM_PROVIDER=fake, {os.environ['LLM_FAKE_SCRIPT']}"

    with databases() as (database_url, redis_url), contextlib.suppress(KeyboardInterrupt):
        asyncio.run(serve(args.port, database_url, redis_url, llm))


if __name__ == "__main__":
    main()
