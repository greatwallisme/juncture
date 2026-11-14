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

/// Build the wire tool name for a tool on a given server.
#[must_use]
pub fn qualified_name(server_alias: &str, tool_name: &str) -> String {
    format!("{server_alias}{QUALIFIER_SEPARATOR}{tool_name}")
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
