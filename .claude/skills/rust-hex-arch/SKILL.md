---
name: rust-hex-arch
description: "Implement hexagonal (ports and adapters) architecture in Rust with clean separation between business logic and external dependencies. Use when: (1) User explicitly asks for `hexagonal architecture`, `ports and adapters`, or `clean architecture` in Rust, (2) Building testable services requiring clear boundaries between domain and infrastructure, (3) Multi-team projects (3+ developers) where coordination needs clear contracts, (4) Projects with expected lifetime 12+ months, (5) Domain-driven design with bounded contexts requiring isolation, (6) Applications where requirements change frequently and adapters need swapping."
---

# Rust Hexagonal Architecture Implementation

Implement hexagonal (ports and adapters) architecture in Rust, ensuring clean separation between business logic and external dependencies.

## Critical Pre-Check: Is Hexagonal Architecture Right for This Project?

**MANDATORY - READ ENTIRE FILE**:
[`project_structure_setup.md`](./project_structure_setup.md) (~16 lines)
**Trigger**: User mentions "hexagonal", "ports and adapters", or "clean architecture"
**Do NOT load**: Implementation files until architecture decision is confirmed

Hexagonal architecture has REAL costs. Know what you're paying:

| Scenario | Use Hexagonal? | Reason |
|----------|----------------|--------|
| Team size 3+ developers | YES | Coordination needs clear boundaries |
| Expected lifetime 12+ months | YES | Architecture overhead pays off over time |
| Requirements change frequently | YES | Easy to swap adapters without touching business logic |
| Simple CRUD app | NO | Use layered architecture instead |
| Prototype/MVP (<3 months) | NO | You'll rewrite anyway |
| Solo learning project | NO | You won't see the benefits |

**DECISION TREE**: If any red flag applies, recommend simpler architecture instead.

---

## Phase 1: Project Structure

**ALREADY LOADED**: `project_structure_setup.md` from pre-check

Create the canonical hexagonal directory structure before writing any code.

---

## Phase 2: Domain Layer (Core Business Logic)

### Step 2.1: Domain Models with Type-Safe Validation

**MANDATORY - READ ENTIRE FILE**:
[`domain_models_with_validation.md`](./domain_models_with_validation.md) (~116 lines)
**Trigger**: User mentions "entities", "models", "validation", "domain types", or "newtype pattern"
**Do NOT load**: http_handler.md, repository_adapter_implementation.md (until later phases)

**Key principle**: Validation happens in constructors, not in services or handlers.

### Step 2.2: Port Definitions (Traits)

**MANDATORY - READ ENTIRE FILE**:
[`domain_traits.md`](./domain_traits.md) (~75 lines)
**Trigger**: User mentions "traits", "ports", "interfaces", or "contracts"
**Do NOT load**: Implementation details until ports are defined

**Trade-off decision**:
- One port per operation? → No, group by domain entity
- One giant port? → No, split by concern (Repository, Service, Metrics)

### Step 2.3: Domain Error Handling

**MANDATORY - READ ENTIRE FILE**:
[`domain_error_handling.md`](./domain_error_handling.md) (~88 lines)
**Trigger**: User mentions "errors", "result types", or "error handling"
**Key principle**: Never use `panic!` or `unwrap()` in domain code. All errors must be recoverable.

---

## Phase 3: Service Implementation

**MANDATORY - READ ENTIRE FILE**:
[`service_implementation.md`](./service_implementation.md) (~143 lines)
**Trigger**: User mentions "services", "business logic", "orchestration", or "use cases"
**Do NOT load**: http_handler.md until Phase 5

**Decision framework**: When logic belongs in Service vs Handler:

| Logic Location | Service | Handler |
|----------------|---------|---------|
| Business rules | YES | NO |
| Multiple adapter coordination | YES | NO |
| HTTP status mapping | NO | YES |
| Request/response conversion | NO | YES |

---

## Phase 4: Adapter Implementations

### Step 4.1: Repository Adapter (Database)

**MANDATORY - READ ENTIRE FILE**:
[`repository_adapter_implementation.md`](./repository_adapter_implementation.md) (~292 lines)
**Trigger**: User mentions "database", "repository", "persistence", "sqlite", "postgres"
**Do NOT load**: HTTP handlers or services

**Trade-off**: sqlx macro vs manual queries
- `sqlx::query!` macro → Compile-time verification, slower compiles
- `sqlx::query` runtime → Faster compiles, runtime errors possible
- Recommendation: Use macros for schemas that change rarely

### Step 4.2: Database Migrations

**LOAD WHEN**: Schema evolution is needed
[`database_migrations.md`](./database_migrations.md) (~272 lines)
**Trigger**: User mentions "migrations", "schema changes", or "zero-downtime"

### Step 4.3: Other Adapters

Load implementation files as needed:
- Metrics adapters (Prometheus, statsd)
- Notification adapters (Email, SMS, webhooks)
- Cache adapters (Redis, in-memory)

---

## Phase 5: Inbound Adapters (Entry Points)

### Step 5.1: HTTP Handler

**MANDATORY - READ ENTIRE FILE**:
[`http_handler.md`](./http_handler.md) (~279 lines)
**Trigger**: User mentions "HTTP", "handlers", "routes", "endpoints", or "axum"
**Do NOT load**: Database adapters or business logic

**Critical anti-patterns**:
```rust
// NEVER: Inject concrete types
pub async fn handler(State(pool): State<SqlitePool>) { }

// ALWAYS: Inject trait objects
pub async fn handler(State(service): State<Arc<dyn AuthorService>>) { }
```

---

## Phase 6: Application Bootstrap

**MANDATORY - READ ENTIRE FILE**:
[`application_bootstrap.md`](./application_bootstrap.md) (~110 lines)
**Trigger**: User mentions "main.rs", "wiring", "dependency injection", or "bootstrap"
**Key principle**: All wiring happens in ONE place (`main.rs`).

---

## Phase 7: Testing

### Step 7.1: Unit Testing with Mocks

**MANDATORY - READ ENTIRE FILE**:
[`unit_testing_services_with_mocks.md`](./unit_testing_services_with_mocks.md) (~147 lines)
**Trigger**: User mentions "unit tests", "mocks", or "testing services"
**Do NOT load**: Integration testing until unit tests are written

### Step 7.2: Integration Testing

**LOAD WHEN**: Verifying database integration
[`integration_testing_with_real_database.md`](./integration_testing_with_real_database.md) (~79 lines)
**Trigger**: User mentions "integration tests" or "real database"

### Step 7.3: Multi-Domain Testing

**LOAD WHEN**: Testing interactions between bounded contexts
[`multi_domain_testing.md`](./multi_domain_testing.md) (~384 lines)
**Trigger**: User mentions "cross-domain", "multiple contexts", or "inter-domain tests"

---

## Phase 8: Performance & Profiling

**LOAD WHEN**: Optimizing performance or investigating bottlenecks
[`profiling_techniques.md`](./profiling_techniques.md) (~362 lines)
**Trigger**: User mentions "performance", "profiling", "flamegraph", or "optimization"

**Key principle**: Profile before optimizing. Most bottlenecks are not where you expect.

---

## Common Pitfalls (CRITICAL)

**MANDATORY - READ ENTIRE FILE**:
[`common_pitfalls_to_avoid.md`](./common_pitfalls_to_avoid.md) (~165 lines)
**Trigger**: Before writing ANY handler or service code

Contains 10 critical anti-patterns with WHY explanations:
1. Leaking implementation details in handlers
2. Orchestration in HTTP handlers
3. Option<T> for required vs optional state
4. Domain entities implementing Serialize/Deserialize
5. Panic/unwrap in domain code
6. Repository returning database errors
7. Validation in services instead of constructors
8. One giant port trait
9. Direct database access in handlers
10. Testing against real database in unit tests

---

## Migration Strategy

For existing codebases: migrate one bounded context at a time. Define ports first, extract business logic to services, add tests, then stabilize. **NEVER rewrite everything at once**.

Use Strangler Fig pattern: old and new code coexist, gradually shift traffic via routing.

---

## Domain Boundaries

Start with one domain per major business function. Do NOT prematurely split into microservices.

**Split when**: (1) Independent deployment, (2) Different release cycles, (3) No critical failure dependencies, (4) Different team ownership.

**Rule**: Split when team communication becomes the bottleneck, not before.

**Symptoms of over-splitting**: Chatty APIs (100+ calls/req), duplicate logic, circular dependencies. Remedy: merge domains.

---

## Performance

**Costs**: Dynamic dispatch (~5-10ns/trait), heap allocation (Arc/Box cache pressure), indirection layers. **When to care**: >10K req/sec. Otherwise don't optimize prematurely.

**Arc vs Rc**: Use Rc for single-threaded tests, Arc for multi-threaded production.

---

## Dependencies

**Load when needed**:
[`essential_dependencies.md`](./essential_dependencies.md) (~69 lines)

**Key decision**: async-trait vs futures
- `async-trait` → Required for trait methods with async
- Bare futures → Works for concrete structs only
