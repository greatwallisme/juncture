//! Prompt-cache prefix stability: canonical JSON normalization, prefix
//! segmentation, and cache-breakpoint planning (design `12-mcp-and-prefix-cache`
//! §2.4, §2.7, §2.9, §2.10).
//!
//! The prompt cache is a hash over the *serialized bytes* of the request, not
//! over Rust values. Every helper in this module therefore has one job: make the
//! bytes a pure function of the semantic content, so two logically identical
//! requests produce identical prefixes.
//!
//! # Scope
//!
//! This module owns exactly one normalization implementation and one breakpoint
//! planner. Providers call them instead of re-deriving the rules, which is what
//! keeps Anthropic and `OpenAI` from drifting apart.

use serde_json::Value;

use crate::llm::ToolDefinition;

/// Cache time-to-live for an explicit breakpoint.
///
/// Anthropic prices a 1h write at 2x the 5m write, so the TTL is a deliberate
/// placement decision rather than a global default: long TTLs belong on the
/// segments that change least (design `12` §2.7 rule 1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    /// Default 5-minute ephemeral cache.
    #[default]
    FiveMinutes,
    /// Extended 1-hour cache (write costs 2x).
    OneHour,
}

impl CacheTtl {
    /// Anthropic wire spelling of the TTL value.
    #[must_use]
    pub const fn as_anthropic_str(self) -> Option<&'static str> {
        match self {
            // 5m is the Anthropic default; sending it explicitly is harmless
            // but `None` keeps the request body minimal.
            Self::FiveMinutes => None,
            Self::OneHour => Some("1h"),
        }
    }
}

/// Explicit cache breakpoint marker attached to a content block.
///
/// Both Anthropic (`cache_control`) and `OpenAI` (`prompt_cache_breakpoint`)
/// mark a *content block*; neither accepts a request-level breakpoint list.
/// Keeping the marker on the content is what lets one abstraction serve both
/// providers (design `12` §2.5).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct CacheControl {
    /// Breakpoint lifetime.
    pub ttl: CacheTtl,
}

impl CacheControl {
    /// Create a breakpoint with the given TTL.
    #[must_use]
    pub const fn new(ttl: CacheTtl) -> Self {
        Self { ttl }
    }

    /// Default 5-minute breakpoint.
    #[must_use]
    pub const fn ephemeral() -> Self {
        Self {
            ttl: CacheTtl::FiveMinutes,
        }
    }

    /// Extended 1-hour breakpoint.
    #[must_use]
    pub const fn one_hour() -> Self {
        Self {
            ttl: CacheTtl::OneHour,
        }
    }
}

/// Segment of the wire request, in Anthropic's cache coverage order
/// (`tools` -> `system` -> `messages`).
///
/// A breakpoint covers everything *before* it, so a breakpoint is only ever
/// placed at the end of a segment. The volatile `Turn` segment must never carry
/// one: it changes on every request, so a breakpoint there would pay a cache
/// write and immediately invalidate it (design `12` §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrefixSegment {
    /// S1: tool definitions (including MCP tools).
    Tools,
    /// S2: system prompt (business background, output-format constraints).
    System,
    /// S3: completed conversation turns (append-only).
    History,
    /// S4: this request's new content (user input, tool results).
    Turn,
}

/// A planned cache breakpoint: which segment ends here, with what TTL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    /// Segment whose end this breakpoint covers.
    pub segment: PrefixSegment,
    /// Breakpoint lifetime.
    pub ttl: CacheTtl,
}

/// Default TTL for a segment.
///
/// S1/S2 are tenant-level content reused across sessions and change rarely, so
/// they take the 1h write. S3 is session-level, where a rolling 5m window is
/// already enough and the 2x write price is not worth paying.
#[must_use]
pub const fn default_ttl(segment: PrefixSegment) -> CacheTtl {
    match segment {
        PrefixSegment::Tools | PrefixSegment::System => CacheTtl::OneHour,
        PrefixSegment::History | PrefixSegment::Turn => CacheTtl::FiveMinutes,
    }
}

/// Minimum cacheable prompt length in tokens for a model.
///
/// Anthropic silently ignores a breakpoint below the threshold and charges
/// nothing, so a sub-threshold breakpoint is a pure waste of one of the four
/// allowed slots. Values come from the published per-model table; unknown
/// models use the conservative 4096 floor.
#[must_use]
pub fn min_cacheable_tokens(model: &str) -> u64 {
    // Ordered from most specific to least: model families share thresholds.
    if model.contains("claude-opus-4") || model.contains("claude-sonnet-4") {
        return 1024;
    }
    if model.contains("claude-3-7-sonnet") || model.contains("claude-3-5-sonnet") {
        return 1024;
    }
    if model.contains("claude-3-5-haiku") {
        return 2048;
    }
    if model.contains("claude-3-opus") || model.contains("claude-3-sonnet") {
        return 1024;
    }
    if model.contains("claude-3-haiku") {
        return 2048;
    }
    // Conservative floor for anything not in the table (including OpenAI,
    // whose documented minimum is 1024).
    4096
}

/// Maximum number of explicit breakpoints the planner will ever emit.
///
/// Anthropic allows 4 per request. The fourth slot is deliberately reserved for
/// the provider's own automatic caching and for debugging an anomalous prefix,
/// so at most 3 are planned (design `12` §2.7 rule 3).
pub const MAX_PLANNED_BREAKPOINTS: usize = 3;

/// Plan explicit cache breakpoints for a request.
///
/// `segment_tokens` reports the token count of each segment that the caller
/// intends to send. A segment receives a breakpoint only when:
///
/// 1. it is not [`PrefixSegment::Turn`] (never mark volatile content), and
/// 2. its cumulative length reaches the model's minimum cacheable length.
///
/// # Arguments
///
/// * `model` - Model name used to look up the minimum cacheable length.
/// * `segment_tokens` - Token count per segment, in S1..S4 order.
///
/// # Returns
///
/// At most [`MAX_PLANNED_BREAKPOINTS`] breakpoints, ordered by segment.
#[must_use]
pub fn plan_breakpoints(model: &str, segment_tokens: &[(PrefixSegment, u64)]) -> Vec<Breakpoint> {
    let minimum = min_cacheable_tokens(model);
    let mut cumulative = 0_u64;
    let mut planned = Vec::new();

    for &(segment, tokens) in segment_tokens {
        cumulative = cumulative.saturating_add(tokens);
        if segment == PrefixSegment::Turn {
            continue;
        }
        if cumulative < minimum {
            continue;
        }
        planned.push(Breakpoint {
            segment,
            ttl: default_ttl(segment),
        });
    }

    planned.truncate(MAX_PLANNED_BREAKPOINTS);
    planned
}

/// Normalize a JSON value so serialization is byte-stable.
///
/// Two independent drifts are corrected:
///
/// - **Key order.** Sorts every object's keys lexicographically. `serde_json`
///   without `preserve_order` already uses a `BTreeMap`, but this function does
///   not rely on that hidden build detail: opening `preserve_order` would
///   silently switch `Value` to insertion order and break every cache.
/// - **Integer-valued floats.** `1.0` and `1` serialize to different bytes.
///   Schema defaults are the usual source, so whole floats collapse to the
///   integer form.
///
/// Everything else (strings, arrays, booleans, null) is preserved exactly.
#[must_use]
pub fn normalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = serde_json::Map::with_capacity(map.len());
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for key in keys {
                if let Some(inner) = map.get(key) {
                    sorted.insert(key.clone(), normalize_json(inner));
                }
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_json).collect()),
        Value::Number(number) => {
            // Already an integer form: keep the exact bytes.
            if number.is_i64() || number.is_u64() {
                return Value::Number(number.clone());
            }
            // Collapse whole floats (`1.0`) to integers so equal values produce
            // equal bytes. The cast is guarded by the range check below; values
            // outside i64 cannot be schema defaults in practice and are left as
            // floats rather than silently truncated.
            if let Some(float) = number.as_f64()
                && float.fract() == 0.0
                && (-9_007_199_254_740_992.0..=9_007_199_254_740_992.0).contains(&float)
            {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the range guard above bounds the value to exactly representable i64s"
                )]
                let as_i64 = float as i64;
                return Value::Number(serde_json::Number::from(as_i64));
            }
            Value::Number(number.clone())
        }
        other => other.clone(),
    }
}

/// Normalize a tool definition's parameter schema in place.
///
/// The tool table is the *first* segment of the Anthropic prefix, so a single
/// unsorted key anywhere in one schema invalidates the whole request's cache.
#[must_use]
pub fn normalize_tool_definition(definition: &ToolDefinition) -> ToolDefinition {
    ToolDefinition {
        name: definition.name.clone(),
        description: definition.description.clone(),
        parameters: normalize_json(&definition.parameters),
    }
}

/// Sort and normalize tool definitions into wire order.
///
/// Ordering is by `name` ascending, with normalization applied first so equal
/// schemas remain byte-equal regardless of how the caller built them.
///
/// This is intentionally *not* part of [`crate::llm::ChatModel::bind_tools`]:
/// a trait implementor must not override the caller's ordering, or a
/// middleware-wrapped model would silently diverge from a bare one.
#[must_use]
pub fn ordered_tool_definitions(definitions: &[ToolDefinition]) -> Vec<ToolDefinition> {
    let mut ordered: Vec<ToolDefinition> =
        definitions.iter().map(normalize_tool_definition).collect();
    // Stable by construction: names are unique within a tool set, but
    // `sort_by` keeps this deterministic even if a caller violates that.
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    ordered
}

/// Estimate token count from a serialized byte length.
///
/// The workspace has no provider tokenizer, and adding one would mean vendoring
/// a large vocabulary per model family. A ~4-bytes-per-token estimate is the
/// documented heuristic and is used **only** for the minimum-length gate: it
/// decides whether a breakpoint is worth emitting, never what content is sent.
/// Being slightly conservative is safe because a sub-threshold breakpoint is
/// silently ignored by the provider anyway.
#[must_use]
pub const fn estimate_tokens(byte_len: usize) -> u64 {
    // ceil(byte_len / 4), kept in integer arithmetic. `usize -> u64` is a
    // widening conversion on every supported target.
    (byte_len as u64).saturating_add(3) / 4
}

/// Stable 64-bit hash of prefix bytes, used for drift detection.
///
/// `xxh3` is already a workspace dependency and is deterministic across
/// processes (unlike `DefaultHasher`, whose seed is not guaranteed stable).
#[must_use]
pub fn hash_prefix(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// Byte-stable serialization of the prefix segments a provider will send.
///
/// Used both for the observability hash (design `12` §2.8 item 3) and for the
/// stability tests in §2.9: two logically equal prefixes must produce equal
/// bytes.
#[must_use]
pub fn prefix_bytes(tools: &[ToolDefinition], system: &str, history: &[String]) -> Vec<u8> {
    let normalized_tools: Vec<ToolDefinition> = ordered_tool_definitions(tools);

    let payload = serde_json::json!({
        "tools": normalized_tools,
        "system": system,
        "history": history,
    });

    // `to_vec` cannot fail for a `Value` tree built from owned data.
    serde_json::to_vec(&payload).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, parameters: Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            parameters,
        }
    }

    #[test]
    fn test_normalize_json_sorts_keys() {
        let unsorted = json!({"z": 1, "a": 2, "m": {"y": 1, "b": 2}});
        let normalized = normalize_json(&unsorted);
        let serialized = serde_json::to_string(&normalized).expect("serialize");
        assert_eq!(serialized, r#"{"a":2,"m":{"b":2,"y":1},"z":1}"#);
    }

    #[test]
    fn test_normalize_json_collapses_whole_floats() {
        let value = json!({"temperature": 1.0, "top_p": 0.5, "count": 3});
        let normalized = normalize_json(&value);
        assert_eq!(
            serde_json::to_string(&normalized).expect("serialize"),
            r#"{"count":3,"temperature":1,"top_p":0.5}"#
        );
    }

    #[test]
    fn test_normalize_json_preserves_arrays_order() {
        let value = json!({"items": [3, 1, 2]});
        let normalized = normalize_json(&value);
        assert_eq!(
            serde_json::to_string(&normalized).expect("serialize"),
            r#"{"items":[3,1,2]}"#
        );
    }

    #[test]
    fn test_normalize_json_is_idempotent() {
        let value = json!({"nested": {"b": [1.0, {"d": 1, "c": 2}], "a": null}});
        let once = normalize_json(&value);
        let twice = normalize_json(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn test_ordered_tool_definitions_ignores_insertion_order() {
        let a = tool(
            "alpha",
            json!({"type": "object", "properties": {"z": {}, "a": {}}}),
        );
        let b = tool("beta", json!({"type": "object", "properties": {"q": {}}}));

        let forward = ordered_tool_definitions(&[a.clone(), b.clone()]);
        let reverse = ordered_tool_definitions(&[b, a]);

        assert_eq!(
            serde_json::to_vec(&forward).expect("serialize forward"),
            serde_json::to_vec(&reverse).expect("serialize reverse")
        );
        assert_eq!(forward[0].name, "alpha");
        assert_eq!(forward[1].name, "beta");
    }

    #[test]
    fn test_prefix_bytes_stable_across_tool_insertion_order() {
        let tool_a = tool(
            "a_tool",
            json!({"type": "object", "properties": {"x": {"default": 1.0}}}),
        );
        let tool_b = tool("b_tool", json!({"type": "object", "properties": {"y": {}}}));

        let first = prefix_bytes(
            &[tool_a.clone(), tool_b.clone()],
            "system prompt",
            &["hi".into()],
        );
        let second = prefix_bytes(&[tool_b, tool_a], "system prompt", &["hi".into()]);

        assert_eq!(first, second);
    }

    #[test]
    fn test_prefix_bytes_changes_when_tool_set_changes() {
        let tool_a = tool("a_tool", json!({"type": "object"}));
        let tool_b = tool("b_tool", json!({"type": "object"}));

        let before = prefix_bytes(std::slice::from_ref(&tool_a), "s", &[]);
        let after = prefix_bytes(&[tool_a, tool_b], "s", &[]);

        assert_ne!(before, after);
    }

    #[test]
    fn test_plan_breakpoints_skips_turn_segment() {
        let plan = plan_breakpoints(
            "claude-sonnet-4-20250514",
            &[
                (PrefixSegment::Tools, 5000),
                (PrefixSegment::System, 1000),
                (PrefixSegment::History, 1000),
                (PrefixSegment::Turn, 10_000),
            ],
        );

        assert!(plan.iter().all(|b| b.segment != PrefixSegment::Turn));
        assert!(!plan.is_empty(), "expected at least one breakpoint");
    }

    #[test]
    fn test_plan_breakpoints_caps_at_three() {
        let plan = plan_breakpoints(
            "claude-sonnet-4-20250514",
            &[
                (PrefixSegment::Tools, 5000),
                (PrefixSegment::System, 5000),
                (PrefixSegment::History, 5000),
            ],
        );
        assert_eq!(plan.len(), MAX_PLANNED_BREAKPOINTS);
    }

    #[test]
    fn test_plan_breakpoints_below_threshold_is_empty() {
        let plan = plan_breakpoints(
            "claude-sonnet-4-20250514",
            &[(PrefixSegment::Tools, 100), (PrefixSegment::System, 100)],
        );
        assert!(plan.is_empty(), "no segment reaches the threshold");
    }

    #[test]
    fn test_plan_breakpoints_uses_cumulative_length() {
        // Each segment alone is below 1024, but tools+system together cross it.
        let plan = plan_breakpoints(
            "claude-sonnet-4-20250514",
            &[(PrefixSegment::Tools, 600), (PrefixSegment::System, 600)],
        );
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].segment, PrefixSegment::System);
    }

    #[test]
    fn test_plan_breakpoints_ttl_per_segment() {
        let plan = plan_breakpoints(
            "claude-sonnet-4-20250514",
            &[(PrefixSegment::Tools, 2000), (PrefixSegment::History, 2000)],
        );
        assert_eq!(plan[0].ttl, CacheTtl::OneHour);
        assert_eq!(plan[1].ttl, CacheTtl::FiveMinutes);
    }

    #[test]
    fn test_min_cacheable_tokens_known_and_unknown_models() {
        assert_eq!(min_cacheable_tokens("claude-sonnet-4-20250514"), 1024);
        assert_eq!(min_cacheable_tokens("claude-3-5-haiku-20241022"), 2048);
        assert_eq!(min_cacheable_tokens("some-future-model"), 4096);
    }

    #[test]
    fn test_cache_ttl_anthropic_spelling() {
        assert_eq!(CacheTtl::FiveMinutes.as_anthropic_str(), None);
        assert_eq!(CacheTtl::OneHour.as_anthropic_str(), Some("1h"));
    }
}
