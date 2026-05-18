# Rust Anti-Patterns Catalog

Reference for common anti-patterns to avoid. Each entry shows "NEVER write X. ALWAYS write Y instead."

---

## Error Handling Anti-Patterns

### unwrap_used

**NEVER write:**
```rust,ignore
let val = map.get("key").unwrap();
let val = option.unwrap();
```

**ALWAYS write:**
```rust,ignore
// Use ? for error propagation
let val = map.get("key").ok_or_else(|| Error::NotFound("key".into()))?;

// Use unwrap_or_else for truly unreachable cases with justification
let val = map.get("key").unwrap_or_else(|| unreachable!(
    "key must exist: initialized in setup() with all required values"
));
```

### expect_used

**NEVER write:**
```rust,ignore
let val = option.expect("must exist");
```

**ALWAYS write:**
```rust,ignore
// Use proper error handling
let val = option.ok_or_else(|| Error::InvalidState("value required"))?;

// For truly unreachable cases, use unreachable! with explanation
let val = option.unwrap_or_else(|| unreachable!("context: reason"));
```

### question_mark

**NEVER write:**
```rust,ignore
match opt {
    Some(v) => v,
    None => return Err(e),
}
```

**ALWAYS write:**
```rust,ignore
let Some(v) = opt else { return Err(e) };
```

### manual_let_else

**NEVER write:**
```rust,ignore
if let Some(v) = opt {
    // use v
} else {
    return Err(e);
}
```

**ALWAYS write:**
```rust,ignore
let Some(v) = opt else { return Err(e) };
```

### todo and unimplemented

**NEVER write:**
```rust,ignore
fn process(&self) -> Result<()> {
    todo!()
}

fn validate(&self) -> bool {
    unimplemented!()
}
```

**ALWAYS write:**
```rust,ignore
// Implement the function properly
fn process(&self) -> Result<()> {
    self.validate()?;
    self.execute()?;
    Ok(())
}

// Or return a proper error for not-yet-implemented features
fn process(&self) -> Result<()> {
    Err(Error::NotImplemented("process".into()))
}
```

---

## Iterator Anti-Patterns

### needless_collect

**NEVER write:**
```rust,ignore
let is_empty = iter.collect::<Vec<_>>().is_empty();
let count = items.iter().collect::<Vec<_>>().len();
for item in iter.collect::<Vec<_>>() { ... }
```

**ALWAYS write:**
```rust,ignore
let is_empty = iter.peekable().peek().is_none();
let count = items.iter().count();
for item in iter { ... }
```

### manual_filter_map

**NEVER write:**
```rust,ignore
items.iter()
    .filter(|x| x.is_some())
    .map(|x| x.unwrap())
```

**ALWAYS write:**
```rust,ignore
items.iter().flatten()
```

**NEVER write:**
```rust,ignore
items.iter()
    .filter(|x| pred(x))
    .map(|x| f(x))
```

**ALWAYS write:**
```rust,ignore
items.iter()
    .filter_map(|x| pred(x).then(|| f(x)))
```

### unnecessary_fold

**NEVER write:**
```rust,ignore
let sum = items.iter().fold(0, |a, x| a + x);
let any = items.iter().fold(false, |a, x| a || check(x));
let all = items.iter().fold(true, |a, x| a && check(x));
```

**ALWAYS write:**
```rust,ignore
let sum: i32 = items.iter().sum();
let any = items.iter().any(check);
let all = items.iter().all(check);
```

### vec_init_then_push

**NEVER write:**
```rust,ignore
let mut v = Vec::new();
v.push(a);
v.push(b);
v.push(c);
```

**ALWAYS write:**
```rust,ignore
let v = vec![a, b, c];
```

### range_zip_with_len

**NEVER write:**
```rust,ignore
for i in 0..items.len() {
    let item = items[i];
    // ...
}
```

**ALWAYS write:**
```rust,ignore
for (i, item) in items.iter().enumerate() {
    // ...
}
```

---

## Type and Conversion Anti-Patterns

### cast_possible_truncation

**NEVER write:**
```rust,ignore
let x: u8 = big_u32 as u8;
```

**ALWAYS write:**
```rust,ignore
let x: u8 = u8::try_from(big_u32)?;
```

### cast_lossless

**NEVER write:**
```rust,ignore
let x: i64 = y as i64;
```

**ALWAYS write:**
```rust,ignore
let x: i64 = i64::from(y);
```

### clone_on_ref_ptr

**NEVER write:**
```rust,ignore
let shared = arc.clone();
```

**ALWAYS write:**
```rust,ignore
let shared = Arc::clone(&arc);
```

### clone_on_copy

**NEVER write:**
```rust,ignore
let copy = x.clone(); // where x: i32
```

**ALWAYS write:**
```rust,ignore
let copy = x;
```

### ptr_arg

**NEVER write:**
```rust,ignore
fn process(s: &String) -> usize {
    s.len()
}
```

**ALWAYS write:**
```rust,ignore
fn process(s: &str) -> usize {
    s.len()
}
```

---

## String Anti-Patterns

### useless_format

**NEVER write:**
```rust,ignore
let s = format!("hello");
```

**ALWAYS write:**
```rust,ignore
let s = "hello".to_owned();
```

### to_string_in_format_args

**NEVER write:**
```rust,ignore
format!("{}", x.to_string())
```

**ALWAYS write:**
```rust,ignore
format!("{x}")
```

### inefficient_to_string

**NEVER write:**
```rust,ignore
let s = "hello".to_string();
```

**ALWAYS write:**
```rust,ignore
let s = "hello".to_owned();
```

---

## Struct and Enum Anti-Patterns

### struct_excessive_bools

**NEVER write:**
```rust,ignore
struct Config {
    a: bool,
    b: bool,
    c: bool,
    d: bool,
}
```

**ALWAYS write:**
```rust,ignore
// Use bitflags
bitflags::bitflags! {
    struct Config: u8 {
        const A = 1 << 0;
        const B = 1 << 1;
        const C = 1 << 2;
        const D = 1 << 3;
    }
}

// Or an enum
#[derive(Debug, Clone, Copy)]
enum ConfigMode {
    Abcd,
    Abc,
    Bcd,
    // ...
}
```

### wildcard_enum_match_arm

**NEVER write:**
```rust,ignore
match value {
    VariantA => { /* ... */ }
    _ => { /* ... */ }
}
```

**ALWAYS write:**
```rust,ignore
match value {
    VariantA => { /* ... */ }
    VariantB => { /* ... */ }
    VariantC => { /* ... */ }
}
```

### single_match

**NEVER write:**
```rust,ignore
match opt {
    Some(v) => { /* use v */ }
    None => {}
}
```

**ALWAYS write:**
```rust,ignore
if let Some(v) = opt {
    /* use v */
}
```

### single_match_else

**NEVER write:**
```rust,ignore
match opt {
    Some(v) => { /* use v */ }
    None => return Err(e),
}
```

**ALWAYS write:**
```rust,ignore
let Some(v) = opt else { return Err(e) };
```

---

## Performance Anti-Patterns

### large_stack_arrays

**NEVER write:**
```rust,ignore
let buffer = [0u8; 65536];
```

**ALWAYS write:**
```rust,ignore
let buffer = vec![0u8; 65536];
```

### box_collection

**NEVER write:**
```rust,ignore
let data: Box<Vec<u8>> = Box::new(vec![0, 1, 2]);
```

**ALWAYS write:**
```rust,ignore
let data: Vec<u8> = vec![0, 1, 2];
```

### explicit_iter_loop

**NEVER write:**
```rust,ignore
for x in v.iter() {
    println!("{}", x);
}
```

**ALWAYS write:**
```rust,ignore
for x in &v {
    println!("{}", x);
}
```

### explicit_into_iter_loop

**NEVER write:**
```rust,ignore
for x in v.into_iter() {
    process(x);
}
```

**ALWAYS write:**
```rust,ignore
for x in v {
    process(x);
}
```

---

## Documentation Anti-Patterns

### missing_errors_doc

**NEVER write:**
```rust,ignore
/// Parses the config file.
pub fn parse(path: &Path) -> Result<Config, Error> {
    // ...
}
```

**ALWAYS write:**
```rust,ignore
/// Parses the config file.
///
/// # Errors
///
/// Returns `Error::NotFound` if the file does not exist.
/// Returns `Error::InvalidFormat` if the file cannot be parsed.
pub fn parse(path: &Path) -> Result<Config, Error> {
    // ...
}
```

### missing_panics_doc

**NEVER write:**
```rust,ignore
/// Gets the first element.
pub fn first(&self) -> &T {
    self.data.get(0).expect("non-empty")
}
```

**ALWAYS write:**
```rust,ignore
/// Gets the first element.
///
/// # Panics
///
/// Panics if the collection is empty.
pub fn first(&self) -> &T {
    self.data.get(0).expect("non-empty")
}
```

### must_use_candidate

**NEVER write:**
```rust,ignore
/// Computes the hash of the data.
pub fn hash(&self) -> u64 {
    // ...
}
```

**ALWAYS write:**
```rust,ignore
/// Computes the hash of the data.
#[must_use]
pub fn hash(&self) -> u64 {
    // ...
}
```

---

## Code Quality Anti-Patterns

### too_many_arguments

**NEVER write:**
```rust,ignore
fn create(
    a: String,
    b: String,
    c: String,
    d: String,
    e: String,
    f: String,
    g: String,
    h: String,
) -> Result<()> {
    // ...
}
```

**ALWAYS write:**
```rust,ignore
struct Config {
    a: String,
    b: String,
    c: String,
    d: String,
    e: String,
    f: String,
    g: String,
    h: String,
}

fn create(config: Config) -> Result<()> {
    // ...
}
```

### cognitive_complexity

**NEVER write:**
```rust,ignore
fn process(&self) -> Result<()> {
    if condition_a {
        if condition_b {
            if condition_c {
                // deeply nested
            }
        }
    }
    // ...
}
```

**ALWAYS write:**
```rust,ignore
fn process(&self) -> Result<()> {
    if !condition_a {
        return Ok(());
    }
    self.process_b()?;

    // ...
}

fn process_b(&self) -> Result<()> {
    if !condition_b {
        return Ok(());
    }
    // ...
}
```

### dbg_macro

**NEVER write:**
```rust,ignore
let result = dbg!(calculate());
```

**ALWAYS write:**
```rust,ignore
// For libraries: use proper logging
log::debug!("calculation result: {:?}", result);

// For one-off debugging: remove before commit
let result = calculate();
```

---

## Lint Configuration Anti-Patterns

### NEVER allow lints globally

**NEVER write:**
```rust,ignore
#![allow(dead_code)]
#![allow(clippy::all)]
```

**ALWAYS write:**
```rust,ignore
// Fix the underlying issue, or use item-level allow with justification
#[expect(clippy::default_trait_access, reason = "generated by prost derive")]
#[derive(Message)]
struct ProtoMessage {}
```

### NEVER lower standards in Cargo.toml

**NEVER write:**
```toml,ignore
[lints.clippy]
pedantic = "allow"
```

**ALWAYS write:**
```toml
[lints.clippy]
pedantic = { level = "warn", priority = -1 }
```
