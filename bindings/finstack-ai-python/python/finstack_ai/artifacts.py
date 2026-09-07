"""Typed projections of the existing kernel artifact reference contract."""

from typing import NotRequired, TypedDict

from .callbacks import JsonValue


class BlobReference(TypedDict):
    """Existing kernel blob reference; staging/authorization remains with the agent."""

    id: str
    media_type: str
    length: int
    digest: NotRequired[str]
    name: NotRequired[str]


class ArtifactReference(TypedDict):
    """Durably staged kernel artifact reference, validated by Rust on use."""

    id: str
    kind: str
    blob: BlobReference
    content_digest: str
    scope_digest: str
    metadata: dict[str, JsonValue]
