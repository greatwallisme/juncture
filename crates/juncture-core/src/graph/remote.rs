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

    /// Invoke the remote graph with the given input.
    ///
    /// Delegates to [`GraphClient::invoke`], serializing `input` to JSON and
    /// deserializing the returned state into `S`. This is the primary way to
    /// execute a remote graph (design `index.md`: "远程图调用，跨进程组合").
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`](crate::client::ClientError) on connection failure, serialization failure,
    /// or a non-success server response.
    pub async fn invoke<S>(
        &self,
        input: &S,
        config: Option<crate::client::InvokeConfig>,
    ) -> Result<S, crate::client::ClientError>
    where
        S: serde::Serialize + for<'de> serde::Deserialize<'de> + Sync,
    {
        let input_value = serde_json::to_value(input)?;
        self.client.invoke(input_value, config).await
    }

    /// Get the current state of a thread on the remote graph.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`](crate::client::ClientError) if the thread is not found or the request fails.
    pub async fn get_state<T>(
        &self,
        thread_id: &str,
    ) -> Result<crate::client::StateSnapshot<T>, crate::client::ClientError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        self.client.get_state(thread_id).await
    }

    /// Get the state history of a thread on the remote graph.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`](crate::client::ClientError) if the request fails.
    pub async fn get_state_history<T>(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::client::StateSnapshot<T>>, crate::client::ClientError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        self.client.get_state_history(thread_id, limit).await
    }

    /// Manually update a thread's state on the remote graph.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`](crate::client::ClientError) if the thread is not found or the update fails.
    pub async fn update_state(
        &self,
        thread_id: &str,
        update: serde_json::Value,
        as_node: Option<&str>,
    ) -> Result<(), crate::client::ClientError> {
        self.client.update_state(thread_id, update, as_node).await
    }

    /// Resume a paused (interrupted) thread on the remote graph.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`](crate::client::ClientError) if the resume fails or the server errors.
    pub async fn resume(
        &self,
        thread_id: &str,
        values: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, crate::client::ClientError> {
        self.client.resume(thread_id, values).await
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
