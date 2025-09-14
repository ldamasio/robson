"""Composition root for dependency injection.

Factories here assemble use cases with concrete adapters.
"""

from __future__ import annotations

# Example placeholder factory pattern
_singletons: dict[str, object] = {}

def get_singleton(key: str, factory):
    if key not in _singletons:
        _singletons[key] = factory()
    return _singletons[key]

