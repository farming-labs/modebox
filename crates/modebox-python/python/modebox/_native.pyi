# The Rust extension (src/lib.rs). Values cross as tagged JSON strings; use the
# API in __init__.py instead.
from collections.abc import Awaitable, Callable

def create_engine(config: str) -> int: ...
def drop_engine(id: int) -> None: ...
def run(
    engine_id: int, code: str, tools: str, options: str, dispatch: Callable[[str], Awaitable[str]]
) -> Awaitable[str]: ...
def resume(
    engine_id: int,
    continuation: str,
    resolutions: str,
    tools: str,
    options: str,
    dispatch: Callable[[str], Awaitable[str]],
) -> Awaitable[str]: ...
def declarations(tools: str) -> str: ...
