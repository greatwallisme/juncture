# Python Debugging Playbook

**Core principle:** In Python, where an error appears and where it originates are often far apart. A traceback gives you the symptom location, not the root cause location.

## Classify First, Investigate Second

```
Symptom appears
    │
    ├─ There is a traceback
    │   ├─ Single exception → Section 1: read traceback, trace up to root cause
    │   └─ Contains "During handling..." or "The above exception..." → Section 1: read the FIRST exception
    │
    ├─ No traceback, result is wrong
    │   ├─ Fails in full suite, passes alone → Section 4: test pollution
    │   ├─ Async code is slow or hangs → Section 3: async blocking
    │   └─ Result grows more wrong with each call → Section 2: mutable state trap
    │
    └─ ImportError / AttributeError at import time → Section 5: circular import
```

**NEVER add `try/except` or apply patches before classifying the bug.** Python makes it easy to swallow errors, which makes the root cause even harder to find.

---

## Section 1: Reading Tracebacks

### The most important rule: read from the bottom up

Most people read Python tracebacks top-down. That is wrong. The top of a traceback is the entry point; the bottom is the symptom. The root cause is somewhere between the symptom and the entry point.

```
Traceback (most recent call last):            ← ignore this line
  File "app.py", line 42, in handle_request   ← entry point, not the cause
    result = await db.get_user(user_id)
  File "db.py", line 17, in get_user          ← intermediate call
    return await self.conn.execute(query)
  File "conn.py", line 5, in execute          ← closest to root cause in your code
    return self._cursor.execute(sql)
AttributeError: 'NoneType' object has no attribute 'execute'   ← symptom
```

**How to read it:** Start at `AttributeError` and ask "why is `_cursor` None?" — do not fix `execute`; find who was supposed to initialize `_cursor` and did not. Trace upward frame by frame until you find the place that failed to do what it should have.

### Chained exceptions: the most commonly misread case

```
ValueError: invalid input
During handling of the above exception, another exception occurred:
RuntimeError: failed to process
```

**Rule: the first exception (`ValueError`) is the root cause. The second one happened while trying to handle the first.**

NEVER look only at the last exception. Fixing `RuntimeError` while `ValueError` keeps firing means the real problem is never solved.

### Getting more context

The default traceback does not show local variable values. When investigation stalls, use:

```bash
# pytest --tb=long shows local variables at each frame
pytest --tb=long tests/
```

```python
# Add assertions at the suspicious location to print current values
def get_user(self, user_id):
    assert self.conn is not None, \
        f"conn is None in get_user(user_id={user_id!r}), session={self!r}"
```

### Interactive investigation: pdb

Add `breakpoint()` before the suspicious line — the program pauses there and enters the debugger:

```python
def suspect_function(data):
    breakpoint()
    result = transform(data)
    return result
```

The most useful pdb commands:
- `p expr` — print any expression
- `pp obj` — pretty-print (more readable for dicts and lists)
- `w` — show the current call stack
- `u` / `d` — move up or down frames in the call stack to inspect variables at each level
- `n` — next line (do not step into functions)
- `s` — step into a function
- `c` — continue running

```bash
# In tests: --pdb drops into the debugger on the first failure
pytest --pdb -x tests/
```

**Evidence is sufficient when:** you can say "variable X in function Y should be A but is B, because function Z returned B instead of A."

---

## Section 2: Mutable State Traps

### Why these are hard to spot

Python has several ways to share mutable state that look like local variables but are actually shared globally. The first call is correct; subsequent calls accumulate state from previous calls.

### Mutable default argument

**Symptom:** First call returns correct result; later calls have data accumulated from earlier calls.

```python
# Trap: [] is created once when the function is defined, shared across all calls
def append_to(item, result=[]):
    result.append(item)
    return result

append_to(1)  # [1]
append_to(2)  # [1, 2]  ← expected [2]
```

**Investigation:** Use `id()` to confirm it is the same object:
```python
print(id(result))    # if id is the same across calls, it is the same object
```

**Fix:** Use `None` as the default and create the list inside the function:
```python
def append_to(item, result=None):
    if result is None:
        result = []
    result.append(item)
    return result
```

### Class variable vs instance variable

```python
class MyClass:
    items = []       # class variable: shared across all instances

    def add(self, item):
        self.items.append(item)   # modifies the class variable, not an instance variable
```

**Investigation:** Use `vars(obj)` to inspect the instance dict and `vars(type(obj))` to inspect the class dict. Confirm which level the variable lives at.

### Module-level mutable object

```python
# config.py
settings = {"debug": False}    # all code that imports this module shares the same dict

# somewhere else
from config import settings
settings["debug"] = True        # takes effect globally everywhere settings is used
```

**Investigation:** Trace all mutation sites by patching `__setitem__`:
```python
import traceback
original_setitem = dict.__setitem__

def traced_setitem(self, key, value):
    if self is settings:
        traceback.print_stack()
        print(f"settings[{key!r}] = {value!r}")
    original_setitem(self, key, value)

dict.__setitem__ = traced_setitem
```

---

## Section 3: Async Blocking

### Why this is hard to notice

The event loop is single-threaded. One blocking call freezes all concurrent tasks — but it may only appear as "a certain request got slow" rather than an obvious error.

### Investigation path

**Step 1: Enable asyncio debug mode and let slow callbacks expose themselves**

```bash
PYTHONASYNCIODEBUG=1 python -m pytest tests/ -v -s
```

This prints any callback that takes more than 100ms to execute:
```
Executing <Task finished name='Task-3' coro=<process() done>> took 2.501 seconds
```

Find that task; you have found the blocking source.

**Step 2: If debug mode is not enough, add timing at async function boundaries**

```python
import time, logging
log = logging.getLogger(__name__)

async def suspect_coroutine(item_id):
    t0 = time.monotonic()
    log.debug("enter suspect_coroutine item_id=%s", item_id)

    result = await some_operation()

    elapsed = time.monotonic() - t0
    log.debug("exit suspect_coroutine elapsed=%.3fs", elapsed)
    return result
```

Find the call with an abnormally large elapsed value — that is the blocking point.

**Step 3: Search the code for synchronous blocking calls**

```bash
# All of these are blocking inside async functions
grep -rn "time\.sleep\b\|requests\.\|open(\|\.read()\|\.write(" app/ --include="*.py"
```

**Common blocking sources and fixes:**

| Blocking call | Fix |
|--------------|-----|
| `time.sleep(n)` | `await asyncio.sleep(n)` |
| `requests.get(url)` | `await httpx.AsyncClient().get(url)` |
| Synchronous file IO | `await asyncio.to_thread(open, path)` |
| Synchronous database call | Switch to an async driver (asyncpg, motor, etc.) |
| Any synchronous library call | `await asyncio.to_thread(sync_function, args)` |

### Missing await

```python
async def do_work():
    result = fetch_data()    # forgot await — returns a coroutine object, not the result
    return result
```

**Symptom:** Function returns a `<coroutine object ...>` instead of the actual value.

```bash
# Make Python treat unawaited coroutine warnings as errors
python -W error::RuntimeWarning -m pytest tests/
```

**Evidence is sufficient when:** you can point to the specific line executing a synchronous blocking call in an async context, or the specific coroutine that was never awaited.

---

## Section 4: Test Pollution

### Why this is hard to track down

The cause and effect span two separate tests: test A corrupts global state, test B depends on that state being clean. Running either test alone is fine; the failure only appears when A runs before B.

**NEVER add setup or teardown to the failing test before locating the pollution source.** You would be treating the symptom, not the cause.

### Investigation path

**Step 1: Confirm it is pollution, not something else**

```bash
# Run the failing test alone — does it pass?
pytest tests/test_foo.py::test_failing -v

# If it passes alone but fails in the full suite, it is pollution
pytest tests/ -v
```

**Step 2: Use bisection to locate the polluter**

```bash
# Install pytest-randomly to randomize test order
pip install pytest-randomly

# Run with a fixed seed to reproduce the failure
pytest tests/ --randomly-seed=12345

# Binary search: split the suite in half, find which half contains the polluter
pytest tests/half_a/ tests/test_foo.py::test_failing   # fails → polluter is in half_a
pytest tests/half_b/ tests/test_foo.py::test_failing   # fails → polluter is in half_b
```

```bash
# Or use find-polluter.sh if available in this skill directory
./find-polluter.sh
```

**Step 3: Snapshot global state inside the failing test to find what changed**

```python
import sys

def snapshot_state():
    return {
        name: getattr(mod, '__dirty__', None)
        for name, mod in sys.modules.items()
        if hasattr(mod, '__dirty__')
    }

def test_failing():
    before = snapshot_state()
    # ... test logic ...
    after = snapshot_state()
    changed = {k: (before.get(k), after[k]) for k in after if after[k] != before.get(k)}
    if changed:
        pytest.fail(f"State changed during test: {changed}")
```

**Evidence is sufficient when:** you can say "test X modified variable Y in module Z; test W depends on Y having its initial value."

---

## Section 5: Circular Imports

**Symptom:** `ImportError: cannot import name 'X' from partially initialized module 'Y'`

**Why this is hard:** The error points to the `import` statement that triggered it, but the root cause is a cycle in the module dependency graph. That `import` line is just the trigger point.

### Investigation path

**Step 1: Trace the import order with `-v` mode**

```bash
python -v script.py 2>&1 | grep "import\|from" | head -60
```

Find the cycle: A imports B, B imports A.

**Step 2: Add a stack trace to the problem module to see who is importing it**

```python
# Add temporarily to the top of problem_module.py
import traceback
print(f"=== {__name__} is being imported ===")
traceback.print_stack()
```

**Step 3: Draw the dependency graph and find the cycle**

Common ways to break cycles:

| Method | When to use |
|--------|-------------|
| Move shared code to a third module C | Both A and B need something; extract it independently |
| Lazy import (import inside a function) | The import is only needed at runtime, not at module load time |
| Redesign module boundaries | Circular imports usually signal that responsibilities are wrongly divided |

**NEVER use lazy imports to work around a circular import without analyzing the root cause.** Lazy importing is a bandage; the module design problem remains.

---

## Architecture Red Flags (trigger Phase 4.5)

When these patterns appear, stop fixing the bug and discuss the architecture:

- **Module-level mutable state modified from multiple places** → Global state design problem; test isolation will remain difficult and every fix will be a patch
- **Synchronous database driver used inside an async service** → Not a local problem; the entire service's concurrency is capped; requires migrating the driver
- **Circular imports papered over with lazy imports** → Module responsibility boundaries are wrong; will become harder to maintain as the codebase grows
- **`except Exception: pass` in multiple places** → Errors are silently discarded; any bug investigation can only see symptoms and will never find the root cause