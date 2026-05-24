# TypeScript Debugging Playbook

**Core principle:** TypeScript has two distinct error surfaces — the type system at compile time and JavaScript runtime behavior. Misidentifying which surface a bug lives on sends the investigation in the wrong direction from the start.

## Classify First, Investigate Second

```
Symptom appears
    │
    ├─ tsc / IDE shows red underlines or build fails
    │   └─→ Compile-time type error (Section 1)
    │        — the type system found a contradiction; understand it before touching code
    │
    ├─ Builds fine, but runtime behavior is wrong
    │   ├─ Value is undefined / TypeError at runtime → Section 2: runtime type unsafety
    │   ├─ async function returns wrong value or never resolves → Section 3: async bugs
    │   ├─ Test passes alone, fails in suite → Section 4: test pollution
    │   └─ Behavior differs between tsc output and ts-node / bundler → Section 5: toolchain mismatch
    │
    └─ Types look correct but the wrong overload or narrowing is used
        └─→ Section 1: type inference trap
```

**NEVER suppress a type error with `as any`, `// @ts-ignore`, or a cast before understanding what the error is saying.** These silence the type system, which is the only tool that can catch this class of bug before runtime.

---

## Section 1: Compile-time Type Errors

### Why the type system is help, not obstacle

TypeScript's type checker is proving that your code could fail at runtime under some inputs. Treating type errors as noise to suppress means trading a compile-time message for a runtime crash that is much harder to reproduce and diagnose.

**The right mental model:** a type error is the compiler saying "I cannot guarantee this is safe." Your job is to understand *why* it cannot guarantee it — not to find the shortest path to making the red underline disappear.

### Investigation path

**Step 1: Read the full error, including the chain of "Type X is not assignable to type Y"**

TypeScript errors are often nested. The outermost message describes the conflict; the innermost message names the specific property or type that does not match. Read all the way to the bottom:

```
Argument of type 'User | null' is not assignable to parameter of type 'User'.
  Type 'null' is not assignable to type 'User'.
```

The innermost line (`Type 'null' is not assignable to type 'User'`) is the actual mismatch. The outer line tells you where it was found.

**Step 2: Run tsc directly to see all errors without IDE filtering**

```bash
npx tsc --noEmit              # type-check without emitting, show all errors
npx tsc --noEmit 2>&1 | less  # page through when there are many
```

**Step 3: Fix only the first error, then recompile**

Later errors are often cascading consequences of the first. Fixing type errors from the bottom of the file up, or all at once, creates confusion about what caused what.

**Step 4: Evidence is sufficient when:**
You can explain in your own words what value could appear at runtime that the type system cannot allow. Cannot explain it = do not understand the error yet = do not touch the code.

### Common type errors and their root causes

**`Object is possibly 'undefined'` or `Object is possibly 'null'`**

This is not a type annotation problem — it is the type system telling you there is a code path where this value is absent and you have not handled it.

Do not fix with `!` (non-null assertion). Fix by handling the absent case:
```typescript
// Wrong: suppresses the error, crashes at runtime if undefined
const name = user!.name;

// Correct: handle the case
if (user === undefined) {
    throw new Error(`user not found for id=${userId}`);
}
const name = user.name;
```

**`Property 'X' does not exist on type 'Y'`**

Before adding the property to the type, ask: should this property actually exist here, or am I accessing the wrong object? Often the wrong variable is being used.

**Type narrowing not working as expected**

TypeScript narrows types inside `if` blocks based on type guards. Common cases where narrowing fails:

```typescript
// Narrowing is lost after an async call
if (value !== null) {
    await someAsyncOperation();  // ← TypeScript forgets the narrowing here
    value.doSomething();         // error: value is possibly null
}

// Fix: re-check after await, or capture before
const captured = value;          // captured: T (not null)
if (captured !== null) {
    await someAsyncOperation();
    captured.doSomething();      // safe: captured cannot be reassigned
}
```

**Generic type inference producing `unknown` or `{}`**

When TypeScript infers `unknown` or `{}` for a generic, it means it could not determine the type from context. Provide an explicit type argument:
```typescript
// TypeScript cannot infer T
const result = fetchData();          // result: unknown

// Provide it explicitly
const result = fetchData<User>();    // result: User
```

### Type assertion traps

`as SomeType` is a promise to the compiler, not a runtime check. If the promise is wrong, the bug appears later and is hard to trace back to the assertion:

```typescript
const user = response.data as User;   // no runtime check — if data is malformed,
user.name.toUpperCase();              // this crashes with: Cannot read properties of undefined
```

Investigation: search for `as` casts and check whether each one is guaranteed to be correct at the assertion site, not just "probably correct most of the time."

---

## Section 2: Runtime Type Unsafety

### Why runtime type bugs are hard to find

TypeScript's types are erased at runtime. Values from external sources — API responses, `JSON.parse`, `localStorage`, user input — arrive as `any` or `unknown` and are often cast without validation. The type system cannot protect you here; the bug lives in the gap between the type annotation and the actual runtime value.

### Investigation path

**Step 1: Find where the bad value enters the system**

Runtime TypeErrors almost always originate at a boundary: API call, JSON parse, environment variable read, DOM event. The TypeError appears deep in the call stack, but the root cause is at the boundary where the value entered unvalidated.

Add logging at every boundary until you find the one that lets bad data through:
```typescript
async function fetchUser(id: string): Promise<User> {
    const response = await fetch(`/api/users/${id}`);
    const data = await response.json();
    console.log("raw API response:", JSON.stringify(data)); // log before any cast
    return data as User;                                    // ← is this cast valid?
}
```

**Step 2: Validate at the boundary, not in the middle of business logic**

Use a runtime validator at every external boundary. Zod is the standard choice:
```typescript
import { z } from "zod";

const UserSchema = z.object({
    id: z.string(),
    name: z.string(),
    email: z.string().email(),
});

const raw = await response.json();
const user = UserSchema.parse(raw);  // throws with a clear message if data is wrong
                                      // instead of a confusing TypeError three calls later
```

**Step 3: Locate `any` in the call chain**

```bash
# Find all explicit any usage
grep -rn ": any\|as any\|<any>" src/ --include="*.ts"
```

Each `any` is a place where the type system stops checking. Trace from the TypeError backward through the call chain until you find the `any` that let the bad value flow through undetected.

**Step 4: Evidence is sufficient when:**
You can point to the specific boundary where an unvalidated value entered, and show the specific path it traveled to the TypeError site.

---

## Section 3: Async Bugs

### The three distinct async failure modes

TypeScript async bugs look similar on the surface but have completely different root causes:

| Symptom | Most likely cause |
|---------|------------------|
| Promise never resolves or rejects | Missing `await`, unhandled rejection, or event that never fires |
| Correct value in isolation, wrong value when concurrent | Race condition on shared mutable state |
| Function returns before async work completes | `await` missing inside a callback passed to a synchronous function |

### Missing await: the silent failure

```typescript
async function saveAndNotify(data: Data) {
    saveToDatabase(data);        // forgot await — fire and forget, errors are lost
    sendNotification(data.id);   // may run before save completes
}
```

**Investigation:**
```bash
# TypeScript can warn about floating promises with no-floating-promises rule
npx eslint --rule '{"@typescript-eslint/no-floating-promises": "error"}' src/
```

Also check: `async` callbacks passed to array methods do not behave as expected
```typescript
// Wrong: forEach does not await the async callback
items.forEach(async (item) => {
    await process(item);          // these all run concurrently, forEach returns immediately
});

// Correct: use a for...of loop to run sequentially
for (const item of items) {
    await process(item);
}

// Or Promise.all for concurrent but awaited
await Promise.all(items.map(item => process(item)));
```

### Race condition on shared state

```typescript
// Wrong: two concurrent calls can read the same value and both increment it
async function increment(key: string) {
    const current = await cache.get(key);    // both reads return 0
    await cache.set(key, current + 1);       // both writes set 1, not 2
}
```

**Investigation:** Add a sequence counter to log the operation order:
```typescript
let seq = 0;
async function increment(key: string) {
    const id = ++seq;
    console.log(`[${id}] reading ${key}`);
    const current = await cache.get(key);
    console.log(`[${id}] read ${key}=${current}, writing ${current + 1}`);
    await cache.set(key, current + 1);
    console.log(`[${id}] done`);
}
```

If two `[X] reading` lines appear before either `[X] done`, there is a race.

### Unhandled promise rejection

```typescript
// This error disappears silently in some environments
async function riskyOperation() {
    throw new Error("something went wrong");
}

riskyOperation();   // no await, no .catch() — the rejection is unhandled
```

**Investigation:**
```bash
# Node.js: listen for unhandled rejections during development
node --unhandled-rejections=throw app.js

# Or add at the top of your entry point
process.on('unhandledRejection', (reason, promise) => {
    console.error('Unhandled rejection:', reason);
});
```

**Evidence is sufficient when:** you can identify whether the bug is missing `await`, a race on shared state, or an unhandled rejection — and then point to the specific location.

---

## Section 4: Test Pollution

### Why this is hard to track down

Test pollution in TypeScript projects comes from three sources that look identical at the test failure level: shared module-level state, Jest module cache (module singleton shared across tests), and mock state that is not reset between tests.

**NEVER add `beforeEach` cleanup to the failing test before finding the source.** You may clean the wrong thing.

### Investigation path

**Step 1: Confirm it is pollution**

```bash
# Run the failing test in isolation
npx jest tests/failing.test.ts --no-coverage

# If it passes, run it after the suspected polluter
npx jest tests/suspect.test.ts tests/failing.test.ts --no-coverage --runInBand
```

`--runInBand` forces sequential execution in the same process, which reproduces shared state pollution.

**Step 2: Find what is shared between tests**

The most common sources in TypeScript/Jest projects:

```typescript
// Module-level mutable singleton — shared across all tests in the same Jest worker
const cache = new Map<string, User>();  // ← never reset between tests

// Mock that was not restored
jest.spyOn(UserService.prototype, 'findById').mockResolvedValue(mockUser);
// if afterEach does not restore this, all subsequent tests use the mock
```

**Step 3: Check mock state**

```typescript
// In Jest, mocks accumulate call history unless explicitly cleared
afterEach(() => {
    jest.clearAllMocks();   // clears call counts and return values
    // or
    jest.restoreAllMocks(); // also restores original implementations (for spyOn)
});
```

**Step 4: Isolate module state**

```typescript
// Force Jest to re-import the module fresh for each test
beforeEach(() => {
    jest.resetModules();
    // then re-require the module under test
});
```

**Evidence is sufficient when:** you can run test A followed by test B in the same process and reproduce the failure, and identify specifically what state A leaves behind that B depends on.

---

## Section 5: Toolchain Mismatch

### Why this category exists

TypeScript code goes through multiple transformation steps: `tsc`, `ts-node`, `esbuild`, `webpack`, `babel` with `@babel/preset-typescript`. Different tools implement different subsets of TypeScript and apply different transforms. Code that behaves one way under `ts-node` may behave differently after `tsc` + Node, or inside a bundler.

### Common mismatches and how to investigate

**`esModuleInterop` and default import behavior**

```typescript
// With esModuleInterop: true
import fs from 'fs';          // works

// Without esModuleInterop: true  
import * as fs from 'fs';     // required
```

Check `tsconfig.json` for `esModuleInterop` and verify the bundler uses the same setting.

**`moduleResolution` mismatch**

```bash
# Check what resolution strategy is configured
cat tsconfig.json | grep moduleResolution

# node16 / bundler requires explicit .js extensions in imports
import { foo } from './utils';    // fails under node16 resolution
import { foo } from './utils.js'; // correct for node16
```

**Decorator behavior differs between tsc and babel**

If using `@decorator` syntax: babel's `@babel/plugin-proposal-decorators` uses a different spec stage than `tsc` with `experimentalDecorators`. The runtime behavior of decorators can differ significantly.

Investigation: reproduce the failure using `tsc` output only, without any babel or bundler in the chain. If the bug disappears, the toolchain transform is the cause.

**Step 1: Minimal reproduction outside the toolchain**

```bash
# Compile with tsc only and run with Node directly
npx tsc && node dist/index.js

# Compare to what the bundler produces
npm run build && node dist/bundle.js
```

If the outputs behave differently, the toolchain is transforming code in a way that changes behavior.

**Step 2: Check tsconfig inheritance**

```bash
# Find all tsconfig files — projects often have overlapping configs
find . -name "tsconfig*.json" | grep -v node_modules

# Check what settings each test runner or build tool actually uses
npx tsc --showConfig           # shows the final merged config
```

**Evidence is sufficient when:** you can reproduce the different behavior with and without the toolchain in the chain, and identify which configuration difference causes it.

---

## Architecture Red Flags (trigger Phase 4.5)

When these patterns appear, stop fixing the bug and discuss the architecture:

- **`any` used throughout to avoid type errors** → Type system is providing no safety; runtime bugs will be invisible until they hit production
- **External API responses cast directly without validation** → Any schema change in the API silently corrupts data; add Zod or equivalent at every boundary
- **Mocks not isolated between tests, requiring `--runInBand` to pass** → Test suite is not hermetic; as it grows, failures will become increasingly random
- **Multiple tsconfigs with conflicting settings across the project** → Builds are non-deterministic; behavior depends on which tool runs first
- **`as` casts required at every layer to pass data through** → Data types are mismodeled at the source; fix the type at its definition, not at each usage site
