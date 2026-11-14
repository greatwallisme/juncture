//! Tool-name disambiguation across multiple MCP servers.
//!
//! MCP guarantees tool-name uniqueness only *within* one server, and
//! `serverInfo.name` is not guaranteed unique, so aggregating servers will
//! collide. Qualified names use `__` because the MCP charset
//! (`A-Za-z0-9_-.`) permits it and models read it as a separator rather than as
//! hierarchy (design `12-mcp-and-prefix-cache` §1.8).

/// Separator between the server alias and the tool name.
///
/// `__` is legal in an MCP tool name and reads as a flat join to a model, which
/// avoids the "is this a namespace path?" confusion a `.` invites.
pub const QUALIFIER_SEPARATOR: &str = "__";

/// Normalize a server name into a stable, readable alias.
///
/// Lowercases, keeps only ASCII alphanumerics and `_`, and collapses everything
/// else to `_`. Returns `"server"` for input that normalizes to nothing so an
/// alias is always usable in a tool name.
#[must_use]
pub fn normalize_alias(name: &str) -> String {
    let mut alias = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            alias.push(ch.to_ascii_lowercase());
        } else if !alias.ends_with('_') {
            alias.push('_');
        }
    }
    let alias = alias.trim_matches('_').to_string();
    if alias.is_empty() {
        "server".to_string()
    } else {
        alias
    }
}

/// Assign unique aliases, appending an ordinal to later duplicates.
///
/// Deterministic given the input order, which is why the caller passes server
/// names in stable configuration order.
#[must_use]
pub fn unique_aliases(names: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    names
        .iter()
        .map(|name| {
            let base = normalize_alias(name);
            let count = seen.entry(base.clone()).or_insert(0);
            let alias = if *count == 0 {
                base
            } else {
                format!("{base}{count}")
            };
            *count += 1;
            alias
        })
        .collect()
}

/// Maximum length of a wire tool name.
///
/// Anthropic rejects a tool name longer than 64 characters outright, which
/// fails the whole request rather than the single tool.
pub const MAX_TOOL_NAME_LEN: usize = 64;

/// Build the wire tool name for a tool on a given server.
///
/// Both halves are sanitized into the intersection of the provider charsets
/// (`A-Za-z0-9_-`), because a wire name outside it makes the provider reject the
/// entire request, not just that tool. MCP only *recommends* its charset, and it
/// permits `.`, which Anthropic does not, so a server is free to send a name that
/// would otherwise poison every call.
///
/// Sanitizing is not one-to-one, so a name that had to change (or had to be
/// truncated) carries a short stable suffix derived from the original. Without
/// it, `read.file` and `read_file` would both become `read_file` and one tool
/// would shadow the other. The alias half never contains `__`: [`normalize_alias`]
/// collapses underscore runs, which keeps the first `__` unambiguously the
/// separator and therefore keeps qualified names unique across servers.
#[must_use]
pub fn qualified_name(server_alias: &str, tool_name: &str) -> String {
    let alias = sanitize_alias_component(server_alias);
    let sanitized = sanitize_name_component(tool_name);
    let head = format!("{alias}{QUALIFIER_SEPARATOR}");
    // One hash-suffix format for both reasons a name has to change, so the
    // budget arithmetic is sound regardless of which one applies.
    let hash_suffix = format!("_{:08x}", short_hash(tool_name));

    let keep = MAX_TOOL_NAME_LEN.saturating_sub(head.len());
    if sanitized.len() <= keep {
        if sanitized == tool_name {
            format!("{head}{sanitized}")
        } else {
            // Sanitizing changed the name; the suffix keeps it distinct from a
            // tool whose original name was already the sanitized form.
            format!("{head}{sanitized}{hash_suffix}")
        }
    } else {
        // Truncation also needs the suffix, so reserve room for it up front.
        let budget = keep.saturating_sub(hash_suffix.len());
        let truncated: String = sanitized.chars().take(budget).collect();
        format!("{head}{truncated}{hash_suffix}")
    }
}

/// Sanitize an alias for use as the leading half of a qualified name.
///
/// Collapses `_` runs so the result never contains `__`, and replaces anything
/// outside `A-Za-z0-9_` with a single `_`.
fn sanitize_alias_component(alias: &str) -> String {
    let mut out = String::with_capacity(alias.len());
    for ch in alias.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "server".to_string()
    } else {
        out
    }
}

/// Sanitize a tool name into the provider charset.
fn sanitize_name_component(tool_name: &str) -> String {
    tool_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Stable 32-bit hash used to keep sanitized names distinct.
///
/// `DefaultHasher` is deliberately not used: its seed is not guaranteed stable
/// across processes, so a wire tool name would change between runs and break the
/// prompt-cache prefix.
fn short_hash(value: &str) -> u32 {
    let digest = xxhash_rust::xxh3::xxh3_64(value.as_bytes());
    // Fold to 32 bits: the suffix only has to disambiguate the tools of one
    // server, never to be collision-resistant across the universe of inputs.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a 32-bit name suffix is intentional; truncation is the fold"
    )]
    let folded = digest as u32;
    folded
}

/// Prepend a provenance line to a tool description.
///
/// The model needs to know which server produced a tool to reason about
/// reliability and authorization; the prefix is one stable line so it does not
/// churn the description bytes.
#[must_use]
pub fn annotated_description(server_alias: &str, description: &str) -> String {
    if description.is_empty() {
        format!("[from server {server_alias}]")
    } else {
        format!("[from server {server_alias}]\n{description}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_alias_lowercases_and_replaces() {
        assert_eq!(normalize_alias("My Server!"), "my_server");
        assert_eq!(normalize_alias("github.com"), "github_com");
    }

    #[test]
    fn test_normalize_alias_collapses_runs() {
        assert_eq!(normalize_alias("a  --  b"), "a_b");
    }

    #[test]
    fn test_normalize_alias_empty_falls_back() {
        assert_eq!(normalize_alias(""), "server");
        assert_eq!(normalize_alias("!!!"), "server");
    }

    #[test]
    fn test_unique_aliases_disambiguate_duplicates() {
        let names = vec![
            "github".to_string(),
            "github".to_string(),
            "GitHub".to_string(),
        ];
        let aliases = unique_aliases(&names);
        assert_eq!(aliases, vec!["github", "github1", "github2"]);
    }

    #[test]
    fn test_unique_aliases_keep_distinct_names() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(unique_aliases(&names), vec!["alpha", "beta"]);
    }

    #[test]
    fn test_qualified_name_uses_double_underscore() {
        assert_eq!(
            qualified_name("github", "create_issue"),
            "github__create_issue"
        );
    }

    #[test]
    fn test_qualified_name_parses_back() {
        let name = qualified_name("fs", "read_file");
        let (alias, tool) = name
            .split_once(QUALIFIER_SEPARATOR)
            .expect("separator present");
        assert_eq!(alias, "fs");
        assert_eq!(tool, "read_file");
    }

    #[test]
    fn test_qualified_name_stays_inside_provider_charset() {
        // MCP permits `.` and only recommends its charset; Anthropic permits
        // neither `.` nor spaces and rejects the whole request on a bad name.
        let name = qualified_name("My Server!", "do something/weird.file");
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')),
            "wire tool name left the provider charset: {name}"
        );
        assert!(name.starts_with("my_server__"));
    }

    #[test]
    fn test_qualified_name_is_stable_across_processes() {
        // A sanitized name carries a hash suffix; a non-deterministic hasher
        // would rename the tool between runs and invalidate the S1 prefix.
        assert_eq!(
            qualified_name("srv", "a.b"),
            qualified_name("srv", "a.b"),
            "sanitized names must be deterministic"
        );
    }

    #[test]
    fn test_sanitized_names_do_not_collide() {
        // Both sanitize to `read_file`; they are different tools and must stay
        // distinct or one silently shadows the other.
        assert_ne!(
            qualified_name("fs", "read.file"),
            qualified_name("fs", "read_file")
        );
    }

    #[test]
    fn test_qualified_name_respects_provider_length_limit() {
        let long = "x".repeat(200);
        let name = qualified_name("srv", &long);
        assert!(
            name.len() <= MAX_TOOL_NAME_LEN,
            "tool name length {} exceeds the provider limit",
            name.len()
        );
    }

    #[test]
    fn test_separator_is_unambiguous_across_servers() {
        // Aliases never contain `__`, so the first `__` is always the boundary
        // and two servers cannot produce the same qualified name.
        assert_ne!(qualified_name("a__b", "c"), qualified_name("a", "b__c"));
    }

    #[test]
    fn test_annotated_description_prepends_provenance() {
        assert_eq!(
            annotated_description("fs", "Read a file"),
            "[from server fs]\nRead a file"
        );
    }

    #[test]
    fn test_annotated_description_handles_empty() {
        assert_eq!(annotated_description("fs", ""), "[from server fs]");
    }
}
