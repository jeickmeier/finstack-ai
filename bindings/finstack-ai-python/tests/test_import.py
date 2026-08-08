"""Smoke tests for the placeholder Python package."""


def test_import_finstack_ai() -> None:
    import finstack_ai

    assert finstack_ai.__doc__ is not None
    assert "placeholder" in finstack_ai.__doc__.lower()
