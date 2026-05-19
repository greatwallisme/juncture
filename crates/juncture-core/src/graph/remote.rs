// Remote graph implementation
//
// This module provides the RemoteGraph type for executing graphs
// that are deployed on a remote server.

use crate::client::GraphClient;

/// Remote graph reference
///
/// Represents a graph deployed on a remote server that can be invoked
/// through the client API.
#[derive(Debug)]
pub struct RemoteGraph {
    /// Server endpoint
    endpoint: String,
    /// Graph identifier
    graph_id: String,
    /// Client for communication
    client: GraphClient,
}

impl RemoteGraph {
    /// Create new remote graph reference
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Server endpoint URL
    /// * `graph_id` - Graph identifier
    pub fn new(endpoint: impl Into<String>, graph_id: impl Into<String>) -> Self {
        let endpoint_str = endpoint.into();
        let graph_id_str = graph_id.into();

        Self {
            endpoint: endpoint_str.clone(),
            graph_id: graph_id_str.clone(),
            client: GraphClient::new(
                reqwest::Client::new(),
                format!("{endpoint_str}/graphs/{graph_id_str}"),
                crate::client::AuthConfig::None,
            ),
        }
    }

    /// Get the graph ID
    #[must_use]
    pub fn graph_id(&self) -> &str {
        &self.graph_id
    }

    /// Get the endpoint
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Get the client
    #[must_use]
    pub const fn client(&self) -> &GraphClient {
        &self.client
    }
}

impl Clone for RemoteGraph {
    fn clone(&self) -> Self {
        Self {
            endpoint: self.endpoint.clone(),
            graph_id: self.graph_id.clone(),
            client: GraphClient::new(
                self.client.client().clone(),
                self.client.endpoint().to_string(),
                self.client.auth().clone(),
            ),
        }
    }
}

// Rust guideline compliant 2026-05-19
