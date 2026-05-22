use crate::{State, error::JunctureError};
use std::{collections::HashMap, pin::Pin, sync::Arc};

/// Edge in a Juncture graph
///
/// Edges define the flow of execution between nodes. Fixed edges always route
/// to the same target, while conditional edges use a router function to determine
/// the target based on the current state.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{edge::Edge, State, Node};
/// use std::sync::Arc;
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// // Fixed edge: routes from "node_a" to "node_b"
/// let fixed = Edge::<MyState>::Fixed {
///     from: "node_a".to_string(),
///     to: "node_b".to_string(),
/// };
///
/// // Conditional edge: routes based on state
/// let conditional = Edge::<MyState>::Conditional {
///     from: "router".to_string(),
///     router: Arc::new(|state: &MyState| {
///         Box::pin(async move {
///             Ok(juncture_core::edge::RouteResult::One("target".to_string()))
///         })
///     }),
///     path_map: PathMap::new(),
/// };
/// ```
#[derive(Clone)]
pub enum Edge<S: State> {
    /// Fixed edge that always routes to the same target
    Fixed {
        /// Source node name
        from: String,
        /// Target node name
        to: String,
    },

    /// Conditional edge that routes based on state
    Conditional {
        /// Source node name
        from: String,
        /// Router function to determine target
        router: Arc<dyn Router<S>>,
        /// Mapping of router return values to target node names
        path_map: PathMap,
    },
}

impl<S: State> std::fmt::Debug for Edge<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed { from, to } => f.debug_tuple("Fixed").field(from).field(to).finish(),
            Self::Conditional { from, path_map, .. } => f
                .debug_tuple("Conditional")
                .field(from)
                .field(path_map)
                .finish(),
        }
    }
}

/// Router trait for conditional edge routing
///
/// Routers examine the current state and determine which node(s) to execute next.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{edge::{Router, RouteResult}, State, error::JunctureError};
/// use std::pin::Pin;
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// // Simple router using closure blanket impl
/// let router = |state: &MyState| -> Pin<Box<dyn std::future::Future<Output = Result<RouteResult, JunctureError>> + '_>> {
///     Box::pin(async move {
///         Ok(RouteResult::One("target".to_string()))
///     })
/// };
/// ```
pub trait Router<S: State>: Send + Sync + 'static {
    /// Determine the next node(s) to execute based on current state
    ///
    /// # Errors
    ///
    /// Returns a [`JunctureError`] if routing logic fails.
    fn route(
        &self,
        state: &S,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>>;
}

/// Blanket implementation for sync functions returning &str
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{edge::{Router, RouteResult}, State, error::JunctureError};
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// // Simple closure router
/// let router = |state: &MyState| -> &str {
///     "target"
/// };
/// ```
impl<S, F> Router<S> for F
where
    S: State,
    F: Fn(&S) -> &str + Send + Sync + 'static,
{
    fn route(
        &self,
        state: &S,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        let target = (self)(state).to_string();
        Box::pin(async move { Ok(RouteResult::One(target)) })
    }
}

/// Result of routing computation
///
/// Indicates which node(s) should execute next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteResult {
    /// Route to a single node
    One(String),

    /// Route to multiple nodes in parallel
    Multiple(Vec<String>),
}

impl RouteResult {
    /// Get the target node name if this is a single target
    ///
    /// Returns `Some(&str)` if this is `One`, `None` otherwise.
    #[must_use]
    pub fn as_target(&self) -> Option<&str> {
        match self {
            Self::One(target) => Some(target),
            Self::Multiple(_) => None,
        }
    }

    /// Get all target node names
    ///
    /// Returns an iterator over all targets.
    #[must_use]
    pub fn targets(&self) -> Vec<&str> {
        match self {
            Self::One(target) => vec![target],
            Self::Multiple(targets) => targets.iter().map(String::as_str).collect(),
        }
    }
}

/// Path mapping for conditional edges
///
/// Maps router return values to target node names. Used for validation
/// and graph visualization.
///
/// # Examples
///
/// ```
/// use juncture_core::edge::PathMap;
/// use std::collections::HashMap;
///
/// // From HashMap
/// let mut map = HashMap::new();
/// map.insert("approve".to_string(), "publish".to_string());
/// map.insert("reject".to_string(), "archive".to_string());
/// let path_map = PathMap::from(map);
///
/// // From slice
/// let path_map = PathMap::from(&[("approve", "publish"), ("reject", "archive")][..]);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathMap(HashMap<String, String>);

impl PathMap {
    /// Create a new empty path map
    #[must_use]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Add a path mapping
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    /// Get the target for a given route value
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }

    /// Check if the map contains a key
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Get an iterator over all mappings
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }

    /// Get the number of mappings
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the map is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<HashMap<String, String>> for PathMap {
    fn from(map: HashMap<String, String>) -> Self {
        Self(map)
    }
}

impl From<&[(&str, &str)]> for PathMap {
    fn from(pairs: &[(&str, &str)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

impl<const N: usize> From<&[(&str, &str); N]> for PathMap {
    fn from(pairs: &[(&str, &str); N]) -> Self {
        Self(
            pairs
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

/// Construct a [`PathMap`] from key-value pairs.
///
/// This macro provides ergonomic syntax for creating [`PathMap`] values
/// for conditional edge routing. Each entry maps a router return value
/// to a target node name.
///
/// Both string literals and owned `String` values are supported.
///
/// # Syntax
///
/// ```ignore
/// use juncture_core::path_map;
///
/// let pm = path_map! {
///     "approve" => "publish",
///     "reject"  => "archive",
/// };
/// ```
///
/// A trailing comma is optional but recommended for consistency.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::path_map;
///
/// // With string literals
/// let pm = path_map! {
///     "a" => "x",
///     "b" => "y",
///     "c" => "z",
/// };
/// assert_eq!(pm.len(), 3);
///
/// // With owned String values
/// let key = String::from("route");
/// let val = String::from("target");
/// let pm = path_map! {
///     key => val,
/// };
/// assert_eq!(pm.get("route"), Some(&"target".to_string()));
/// ```
#[macro_export]
macro_rules! path_map {
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut __pm = $crate::PathMap::new();
        $( __pm.insert($key, $value); )+
        __pm
    }};
}

#[cfg(test)]
mod path_map_macro_tests {

    #[test]
    fn test_path_map_macro_str_literals() {
        let pm = path_map! {
            "approve" => "publish",
            "reject" => "archive",
        };
        assert_eq!(pm.get("approve"), Some(&"publish".to_string()));
        assert_eq!(pm.get("reject"), Some(&"archive".to_string()));
        assert_eq!(pm.len(), 2);
    }

    #[test]
    fn test_path_map_macro_trailing_comma() {
        let pm = path_map! {
            "start" => "middle",
            "middle" => "end",
        };
        assert_eq!(pm.get("start"), Some(&"middle".to_string()));
        assert_eq!(pm.get("middle"), Some(&"end".to_string()));
    }

    #[test]
    fn test_path_map_macro_owned_strings() {
        let key = "hello".to_string();
        let val = "world".to_string();
        let pm = path_map! {
            key => val,
        };
        assert_eq!(pm.get("hello"), Some(&"world".to_string()));
    }

    #[test]
    fn test_path_map_macro_single_entry() {
        let pm = path_map! {
            "only" => "entry",
        };
        assert_eq!(pm.len(), 1);
        assert_eq!(pm.get("only"), Some(&"entry".to_string()));
    }
}

// Rust guideline compliant 2026-05-22
