"""Native PR-027 package metadata surface."""

__version__: str
__engine_version__: str

def health() -> str:
    """Return ``"ok"`` without initializing runtime or network resources."""

def build_metadata() -> dict[str, str | bool | int]:
    """Return native build and compatibility metadata."""

def linked_providers() -> tuple[str, ...]:
    """Return curated Rust-backed providers linked into this extension."""
