# Over-Engineering Anti-Patterns

A catalog of patterns that add complexity without proportional value. In a hedge fund, where code is written to serve a strategy that may change in months, over-engineering is the most common and expensive mistake.

## The Litmus Test

Before adding any abstraction, answer these three questions:

1. **Does this solve a problem I have right now?** (Not "might have someday.")
2. **Is the added complexity less than the complexity it removes?**
3. **Would a competent engineer understand this in under 5 minutes?**

If any answer is "no," don't add it.

## Pattern Catalog

### 1. The Lonely Interface

**What it looks like**: An interface/trait with exactly one implementation.

```rust
// BAD: trait with one impl
trait DataLoader {
    fn load(&self, path: &str) -> Result<DataFrame>;
}
struct CsvDataLoader;
impl DataLoader for CsvDataLoader { ... }

// GOOD: just a function
fn load_csv(path: &str) -> Result<DataFrame> { ... }
```

```python
# BAD: ABC with one subclass
class BaseProcessor(ABC):
    @abstractmethod
    def process(self, data): ...


class ActualProcessor(BaseProcessor):
    def process(self, data): ...


# GOOD: just a class (or a function)
class Processor:
    def process(self, data): ...
```

**When the interface IS justified**: When you have 2+ implementations today, or when it's at a system boundary (e.g., swapping between live and paper trading).

### 2. Factory of One

**What it looks like**: A factory that creates exactly one type.

```python
# BAD
class ModelFactory:
    @staticmethod
    def create(model_type: str) -> Model:
        if model_type == "linear":
            return LinearModel()
        raise ValueError(f"Unknown model: {model_type}")


# GOOD
model = LinearModel()
```

**When factories ARE justified**: When object creation is genuinely complex (many dependencies, conditional initialization) and you create different types based on runtime config.

### 3. The Config-Driven Illusion

**What it looks like**: Behavior controlled by configuration that never actually changes.

```python
# BAD: config that's always the same
config = {
    "use_cache": True,  # has literally never been False
    "max_retries": 3,  # has literally never been changed
    "log_level": "INFO",  # toggled once in 2019
}

# GOOD: constants or hardcoded
MAX_RETRIES = 3
USE_CACHE = True
```

**When config IS justified**: When values genuinely change between environments (dev/staging/prod) or when operations staff need to tune without redeploying.

### 4. The Delegation Chain

**What it looks like**: A calls B calls C, and each layer adds nothing.

```python
# BAD
class TradeService:
    def execute(self, trade):
        return self.trade_executor.execute(trade)


class TradeExecutor:
    def execute(self, trade):
        return self.order_manager.submit(trade)


class OrderManager:
    def submit(self, trade):
        return self.broker_client.send(trade)


# GOOD: collapse to what matters
class TradeService:
    def execute(self, trade):
        return self.broker_client.send(trade)
```

### 5. Premature Generic

**What it looks like**: Generic type parameters instantiated with exactly one type.

```rust
// BAD: generic for no reason
struct Pipeline<T: DataSource, U: Transformer, V: Sink> {
    source: T,
    transformer: U,
    sink: V,
}
// Only ever used as Pipeline<CsvSource, CleanTransformer, DbSink>

// GOOD: concrete types
struct Pipeline {
    source: CsvSource,
    transformer: CleanTransformer,
    sink: DbSink,
}
```

**When generics ARE justified**: Library code reused across multiple concrete types. Core data structures (Vec, HashMap). Functions that genuinely operate on different types.

### 6. The Event Bus Nobody Rides

**What it looks like**: An event/message bus for communication between components that could just call each other.

```python
# BAD: pub/sub for two components
event_bus.publish("data_loaded", data)
# ... in another file ...
event_bus.subscribe("data_loaded", self.on_data_loaded)

# GOOD: direct call
processor.handle_loaded_data(data)
```

**When event buses ARE justified**: Truly decoupled systems, cross-process communication, plugin architectures where the consumer set is dynamic.

### 7. The Builder Nobody Builds

**What it looks like**: Builder pattern for objects with 2-3 fields.

```rust
// BAD
let config = ConfigBuilder::new()
    .host("localhost")
    .port(8080)
    .build()?;

// GOOD
let config = Config { host: "localhost".into(), port: 8080 };
```

**When builders ARE justified**: Objects with 5+ optional fields, complex validation during construction, or fluent APIs that genuinely improve readability.

### 8. Repository-Service-Controller for a Script

**What it looks like**: Full enterprise layered architecture for something that runs once a day.

```python
# BAD: 4 files for a daily job
class PositionRepository:
    def get_positions(self): ...


class PositionService:
    def reconcile(self): ...


class PositionController:
    def run_reconciliation(self): ...


class PositionReconciliationApp:
    def main(self): ...


# GOOD: one file, one function
def reconcile_positions():
    positions = db.query("SELECT * FROM positions WHERE date = today()")
    # ... reconciliation logic ...
    db.insert(reconciled)
```

### 9. Wrapper Types That Add Nothing

```python
# BAD
class Price:
    def __init__(self, value: float):
        self.value = value

    def __float__(self):
        return self.value

    def __add__(self, other):
        return Price(self.value + other.value)

    # ... 20 more dunder methods ...


# GOOD (if no invariants to enforce)
price: float = 99.50
```

**When wrapper types ARE justified**: When they enforce invariants (e.g., `NonNegativePrice`), carry units that prevent mixing (e.g., dollars vs. basis points), or provide domain-specific operations that plain types don't.

### 10. The Premature Microservice

**What it looks like**: Splitting a monolith into services before there's a scaling or organizational reason.

If one team owns the code and it runs on one machine, it's a function call, not an API call. Network boundaries add latency, failure modes, and operational complexity.

## Decision Framework

When tempted to add an abstraction, score it:

| Question | Yes = +1, No = -1 |
|----------|-------------------|
| Do I have 2+ concrete uses right now? | |
| Does it reduce total code by >20%? | |
| Does it make the happy path clearer? | |
| Would removing it require touching 3+ files? | |
| Does a team member other than me want it? | |

**Score 3+**: Probably worth it.
**Score 1-2**: Probably not. Reconsider.
**Score ≤0**: Definitely not. Write the simple thing.
