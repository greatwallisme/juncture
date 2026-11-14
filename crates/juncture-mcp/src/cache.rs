//! `tools/list` caching honoring `ttlMs` and `cacheScope`.
//!
//! `ttlMs` governs how long a tool *list* may be reused; it is entirely
//! independent of any LLM prompt-cache TTL and the two must never share a number
//! (design `12-mcp-and-prefix-cache` §1.7, §2.10).
//!
//! `cacheScope` governs *who* may reuse it. A `private` list is bucketed by an
//! authorization fingerprint; putting a private list into the shared bucket
//! would serve one tenant's tools to another.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::protocol::{CacheScope, McpTool};

/// Default lifetime when a server omits `ttlMs`.
///
/// Conservative: an unspecified TTL is treated as "refresh soon" rather than
/// "cache forever", because a stale tool table silently changes the LLM prefix.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// Cache key for one page of a `tools/list` result.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    server_id: String,
    /// `None` for the shared bucket, `Some(fingerprint)` for a private bucket.
    auth_fingerprint: Option<String>,
    method: String,
    cursor: Option<String>,
}

/// One cached page plus its expiry.
#[derive(Clone, Debug)]
struct CacheEntry {
    tools: Vec<McpTool>,
    next_cursor: Option<String>,
    expires_at: Instant,
}

/// One decoded `tools/list` page, carrying the protocol fields that govern how
/// it may be cached.
#[derive(Clone, Debug)]
pub struct Page {
    /// Tools on this page.
    pub tools: Vec<McpTool>,
    /// Pagination cursor for the next page.
    pub next_cursor: Option<String>,
    /// Sharing scope.
    pub scope: CacheScope,
    /// Server-declared lifetime, in milliseconds.
    pub ttl_ms: Option<u64>,
}

/// In-memory `tools/list` cache.
///
/// `ttl_ms` per page is honored independently, matching the protocol: each page
/// carries its own TTL, and a cursor that expires mid-pagination invalidates the
/// whole list (the caller restarts paging).
#[derive(Debug, Default)]
pub struct ToolListCache {
    entries: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl ToolListCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a cached page.
    ///
    /// Returns `None` when absent or expired; an expired entry is evicted.
    ///
    /// # Arguments
    ///
    /// * `server_id` - Stable identifier of the server.
    /// * `scope` - The page's declared scope.
    /// * `auth_fingerprint` - Authorization context, required for `Private`.
    /// * `cursor` - Pagination cursor, `None` for the first page.
    #[must_use]
    pub fn get(
        &self,
        server_id: &str,
        scope: CacheScope,
        auth_fingerprint: Option<&str>,
        cursor: Option<&str>,
    ) -> Option<(Vec<McpTool>, Option<String>)> {
        let key = Self::key(server_id, scope, auth_fingerprint, cursor);
        let now = Instant::now();
        // Take ownership of the entry: an expired one is simply not reinserted,
        // which evicts it without a second lookup or a clone.
        let entry = self.lock().remove(&key)?;
        if now >= entry.expires_at {
            return None;
        }
        Some((entry.tools, entry.next_cursor))
    }

    /// Store one page from an already-decoded `tools/list` payload.
    ///
    /// # Errors
    ///
    /// Same as [`Self::put`].
    pub fn put_page(
        &self,
        server_id: &str,
        auth_fingerprint: Option<&str>,
        cursor: Option<&str>,
        page: Page,
    ) -> Result<(), String> {
        self.put(
            server_id,
            page.scope,
            auth_fingerprint,
            cursor,
            page.tools,
            page.next_cursor,
            page.ttl_ms,
        )
    }

    /// Store one page.
    ///
    /// `ttl_ms` of `None` uses [`DEFAULT_TTL`].
    ///
    /// # Errors
    ///
    /// Returns an error string when a `Private` page is stored without an
    /// authorization fingerprint: such an entry would be indistinguishable from
    /// a shared one and could cross tenants.
    #[allow(
        clippy::too_many_arguments,
        reason = "the low-level primitive mirrors the protocol fields one-to-one; callers use put_page"
    )]
    pub fn put(
        &self,
        server_id: &str,
        scope: CacheScope,
        auth_fingerprint: Option<&str>,
        cursor: Option<&str>,
        tools: Vec<McpTool>,
        next_cursor: Option<String>,
        ttl_ms: Option<u64>,
    ) -> Result<(), String> {
        if scope == CacheScope::Private && auth_fingerprint.is_none() {
            return Err(
                "private tools/list result requires an auth fingerprint; refusing to share it"
                    .to_string(),
            );
        }

        let key = Self::key(server_id, scope, auth_fingerprint, cursor);
        let ttl = ttl_ms.map_or(DEFAULT_TTL, Duration::from_millis);
        let entry = CacheEntry {
            tools,
            next_cursor,
            expires_at: Instant::now() + ttl,
        };
        self.lock().insert(key, entry);
        Ok(())
    }

    /// Drop every page for one server.
    ///
    /// Called when `notifications/tools/list_changed` arrives, which must
    /// invalidate immediately rather than waiting for the TTL (design `12` §1.7).
    pub fn invalidate_server(&self, server_id: &str) {
        self.lock().retain(|key, _| key.server_id != server_id);
    }

    /// Number of live (non-expired) pages. Test-only.
    #[cfg(test)]
    fn live_len(&self) -> usize {
        let now = Instant::now();
        self.lock()
            .values()
            .filter(|entry| entry.expires_at > now)
            .count()
    }

    fn key(
        server_id: &str,
        scope: CacheScope,
        auth_fingerprint: Option<&str>,
        cursor: Option<&str>,
    ) -> CacheKey {
        CacheKey {
            server_id: server_id.to_string(),
            // A public entry deliberately uses the shared bucket even when a
            // fingerprint is supplied: public means public.
            auth_fingerprint: match scope {
                CacheScope::Public => None,
                CacheScope::Private => auth_fingerprint.map(str::to_string),
            },
            method: "tools/list".to_string(),
            cursor: cursor.map(str::to_string),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<CacheKey, CacheEntry>> {
        // A poisoned lock means a previous holder panicked mid-update; the map
        // is still structurally valid, so recovering keeps the cache usable
        // instead of turning one panic into a permanent failure.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        }
    }

    #[test]
    fn test_public_cache_hit() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Public,
                None,
                None,
                vec![tool("a")],
                None,
                Some(60_000),
            )
            .expect("store");

        let (tools, cursor) = cache
            .get("srv", CacheScope::Public, None, None)
            .expect("hit");
        assert_eq!(tools.len(), 1);
        assert!(cursor.is_none());
    }

    #[test]
    fn test_private_entries_are_bucketed_by_auth() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Private,
                Some("tenant-a"),
                None,
                vec![tool("a")],
                None,
                Some(60_000),
            )
            .expect("store");

        assert!(
            cache
                .get("srv", CacheScope::Private, Some("tenant-a"), None)
                .is_some()
        );
        assert!(
            cache
                .get("srv", CacheScope::Private, Some("tenant-b"), None)
                .is_none(),
            "another tenant must never see this entry"
        );
    }

    #[test]
    fn test_private_store_without_fingerprint_is_refused() {
        let cache = ToolListCache::new();
        let result = cache.put(
            "srv",
            CacheScope::Private,
            None,
            None,
            vec![tool("a")],
            None,
            Some(60_000),
        );
        assert!(result.is_err(), "sharing a private list is a data-leak bug");
    }

    #[test]
    fn test_public_entry_shared_despite_fingerprint() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Public,
                Some("tenant-a"),
                None,
                vec![tool("a")],
                None,
                Some(60_000),
            )
            .expect("store");

        assert!(
            cache
                .get("srv", CacheScope::Public, Some("tenant-b"), None)
                .is_some(),
            "public means shared across callers"
        );
    }

    #[test]
    fn test_zero_ttl_is_immediately_expired() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Public,
                None,
                None,
                vec![tool("a")],
                None,
                Some(0),
            )
            .expect("store");
        assert!(cache.get("srv", CacheScope::Public, None, None).is_none());
    }

    #[test]
    fn test_invalidate_server_drops_all_pages() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Public,
                None,
                None,
                vec![tool("a")],
                Some("p2".to_string()),
                Some(60_000),
            )
            .expect("store");

        cache.invalidate_server("srv");
        assert!(cache.get("srv", CacheScope::Public, None, None).is_none());
        assert_eq!(cache.live_len(), 0);
    }

    #[test]
    fn test_invalidate_server_leaves_other_servers() {
        let cache = ToolListCache::new();
        for server in ["a", "b"] {
            cache
                .put(
                    server,
                    CacheScope::Public,
                    None,
                    None,
                    vec![tool("t")],
                    None,
                    Some(60_000),
                )
                .expect("store");
        }
        cache.invalidate_server("a");
        assert!(cache.get("a", CacheScope::Public, None, None).is_none());
        assert!(cache.get("b", CacheScope::Public, None, None).is_some());
    }

    #[test]
    fn test_cursor_partitions_pages() {
        let cache = ToolListCache::new();
        cache
            .put(
                "srv",
                CacheScope::Public,
                None,
                None,
                vec![tool("page1")],
                Some("c2".to_string()),
                Some(60_000),
            )
            .expect("store");
        cache
            .put(
                "srv",
                CacheScope::Public,
                None,
                Some("c2"),
                vec![tool("page2")],
                None,
                Some(60_000),
            )
            .expect("store");

        let (first, cursor) = cache
            .get("srv", CacheScope::Public, None, None)
            .expect("first page");
        assert_eq!(first[0].name, "page1");
        assert_eq!(cursor.as_deref(), Some("c2"));

        let (second, _) = cache
            .get("srv", CacheScope::Public, None, Some("c2"))
            .expect("second page");
        assert_eq!(second[0].name, "page2");
    }
}
