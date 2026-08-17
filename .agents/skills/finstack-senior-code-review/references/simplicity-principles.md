# Simplicity Principles

Heuristics for keeping production code lean, readable, and replaceable.

## The Production Code Mindset

Production code exists to serve current requirements. Requirements change. The code that survives isn't the most "architecturally pure" — it's the code that can be understood, modified, or replaced quickly by whoever inherits it.

**Build for the next person, not the next decade.**

## Principles

### 1. Fewer Files, Fewer Problems

Every new file is a navigation tax. Before creating a new module:

- Does this logic have a natural home in an existing file?
- Will this file have at least 50 lines of meaningful content, or is it a 10-line wrapper?
- Would a new reader intuitively look for this logic in this file?

A 300-line file with clear sections is better than 6 files of 50 lines each glued together by imports.

### 2. Delete Before You Add

The best code is code that doesn't exist.

- Before adding a feature, ask: do we actually need this?
- Before adding an abstraction, ask: does the concrete version cause real problems?
- Before adding a library, ask: is it worth the dependency for what we're using?

If code hasn't been touched in 6 months and isn't on a hot path, it's a candidate for deletion.

### 3. Inline Until It Hurts

Don't extract a function until:

- It's called from 2+ places, OR
- It's complex enough that a descriptive name significantly improves the calling code, OR
- It needs independent testing

A private helper called once, with a name that just restates the code, adds noise.

```python
# BAD: extraction that adds nothing
def _is_valid_price(price):
    return price > 0


def process(price):
    if _is_valid_price(price):
        ...


# GOOD: inline
def process(price):
    if price > 0:
        ...
```

### 4. Flat is Better Than Nested

Deep nesting obscures control flow. Refactor with early returns.

```python
# BAD: deep nesting
def process_record(record):
    if record is not None:
        if record.is_valid():
            if record.size > 0:
                if record.kind:
                    apply(record)
                else:
                    log.error("missing kind")
            else:
                log.error("empty record")
        else:
            log.error("invalid record")


# GOOD: early returns
def process_record(record):
    if record is None:
        return
    if not record.is_valid():
        log.error("invalid record")
        return
    if record.size <= 0:
        log.error("empty record")
        return
    if not record.kind:
        log.error("missing kind")
        return
    apply(record)
```

### 5. Boring Technology

Use boring, proven tools. Exciting new frameworks are someone else's problem.

- PostgreSQL over the NoSQL flavor of the month
- requests over the async HTTP client you saw on Hacker News
- pandas over the "blazingly fast" DataFrame library with 200 GitHub stars
- Standard library over third-party when the standard library is adequate

Adopt new technology when there's a specific, measured performance or capability gap — not because a blog post said it's better.

### 6. One Way to Do It

If there are two ways to accomplish something in the codebase, pick one and migrate the other. Consistency reduces cognitive load.

- One HTTP client library
- One date/time library
- One serialization format for internal data
- One logging pattern
- One error handling convention

### 7. Names That Work Without Comments

If you need a comment to explain a variable or function name, the name is wrong.

```python
# BAD
x = get_data()  # fetches the latest session from the store
transform(x)  # converts to the public handle

# GOOD
session = fetch_latest_session()
handle = to_public_handle(session)
```

### 8. Tests That Prove Behavior, Not Implementation

Test what the code does, not how it does it.

```python
# BAD: testing implementation
def test_uses_cache():
    service = SessionService()
    service.open(spec)
    assert service._cache._store.__len__() == 1  # tied to internals


# GOOD: testing behavior
def test_same_result_on_repeated_call():
    service = SessionService()
    first = service.open(spec)
    second = service.open(spec)
    assert first == second  # tests the contract, not the mechanism
```

### 9. Error Messages That Help You Fix the Problem

```python
# BAD
raise ValueError("invalid input")

# GOOD
raise ValueError(f"Expected positive price, got {price} for {symbol} on {date}")
```

At 2 AM when production is broken, the error message is your only friend. Include: what was expected, what was received, and enough context to find the record.

### 10. Dependencies Are Liabilities

Every dependency is code you didn't write, don't control, and must update.

- Pin versions. `requests>=2.0` is an invitation to a breaking change at the worst time.
- Audit transitive dependencies. Your 5 direct dependencies may pull in 50 packages.
- Prefer standard library alternatives for simple tasks.
- If you only use one function from a library, consider copying that function instead.

## The Simplicity Checklist

Before merging, ask:

- [ ] Could I explain this to a new hire in 5 minutes?
- [ ] Is every file necessary? Every class? Every function?
- [ ] If I came back to this in 6 months, would I understand it immediately?
- [ ] Are there any "just in case" abstractions I can remove?
- [ ] Is there dead code, commented-out code, or unused imports?
- [ ] Does the test suite test behavior, not implementation details?
- [ ] Are error messages actionable?
