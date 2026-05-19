// Juncture client types for remote graph execution
//
// This module provides client types for interacting with Juncture Server
// from external applications.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Authentication configuration for client
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthConfig {
    /// No authentication
    None,
    /// Bearer token authentication
    Token(String),
    /// API key authentication with custom header
    ApiKey {
        /// Header name for the API key
        header: String,
        /// API key value
        key: String,
    },
}

/// Configuration for graph invocation
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InvokeConfig {
    /// Thread ID for stateful execution
    pub thread_id: Option<String>,
    /// Checkpoint ID for time-travel
    pub checkpoint_id: Option<String>,
    /// Recursion limit
    pub recursion_limit: Option<usize>,
    /// Metadata
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Tags
    pub tags: Option<Vec<String>>,
    /// Interrupt before nodes
    pub interrupt_before: Option<Vec<String>>,
    /// Interrupt after nodes
    pub interrupt_after: Option<Vec<String>>,
}

/// Thread information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    /// Thread ID
    pub id: String,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Metadata
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Graph information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphInfo {
    /// Graph ID
    pub id: String,
    /// Graph name
    pub name: String,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// State snapshot
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateSnapshot<T> {
    /// State values
    pub values: T,
    /// Next checkpoint ID
    pub next: Option<String>,
    /// Metadata
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Creation timestamp
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Client error types
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Connection error
    #[error("connection error: {0}")]
    Connection(String),

    /// Authentication failed
    #[error("authentication failed: {0}")]
    Auth(String),

    /// Graph not found
    #[error("graph not found: {0}")]
    GraphNotFound(String),

    /// Thread not found
    #[error("thread not found: {0}")]
    ThreadNotFound(String),

    /// Run not found
    #[error("run not found: {0}")]
    RunNotFound(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    /// HTTP request error
    #[error("HTTP request error: {0}")]
    RequestError(#[from] reqwest::Error),

    /// Server error
    #[error("server error ({status}): {message}")]
    Server {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },

    /// Timeout
    #[error("timeout")]
    Timeout,

    /// Other errors
    #[error("client error: {0}")]
    Other(String),
}

/// Juncture client for server interaction
///
/// Provides methods for managing graphs and threads.
#[derive(Debug)]
pub struct JunctureClient {
    /// HTTP client
    client: reqwest::Client,
    /// Server endpoint
    endpoint: String,
    /// Authentication configuration
    auth: AuthConfig,
}

impl JunctureClient {
    /// Create new client
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Server base URL
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            auth: AuthConfig::None,
        }
    }

    /// Set authentication
    ///
    /// # Arguments
    ///
    /// * `auth` - Authentication configuration
    #[must_use]
    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = auth;
        self
    }

    /// List all deployed graphs
    ///
    /// # Returns
    ///
    /// List of graph information
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the request fails, authentication fails, or the server returns an error.
    pub async fn list_graphs(&self) -> Result<Vec<GraphInfo>, ClientError> {
        let url = format!("{}/graphs", self.endpoint);
        let response = self
            .client
            .get(&url)
            .apply_auth(&self.auth)?
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(ClientError::RequestError)
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get graph client for specific graph
    ///
    /// # Arguments
    ///
    /// * `graph_id` - Graph identifier
    ///
    /// # Errors
    ///
    /// This function cannot fail.
    #[must_use]
    pub fn graph(&self, graph_id: &str) -> GraphClient {
        GraphClient {
            client: self.client.clone(),
            endpoint: format!("{}/graphs/{}", self.endpoint, graph_id),
            auth: self.auth.clone(),
        }
    }

    /// Create new thread
    ///
    /// # Arguments
    ///
    /// * `metadata` - Optional thread metadata
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the request fails, authentication fails, or the server returns an error.
    pub async fn create_thread(
        &self,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<Thread, ClientError> {
        let url = format!("{}/threads", self.endpoint);
        let response = self
            .client
            .post(&url)
            .apply_auth(&self.auth)?
            .json(&serde_json::json!({ "metadata": metadata }))
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(ClientError::RequestError)
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get thread information
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn get_thread(&self, thread_id: &str) -> Result<Thread, ClientError> {
        let url = format!("{}/threads/{}", self.endpoint, thread_id);
        let response = self
            .client
            .get(&url)
            .apply_auth(&self.auth)?
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(ClientError::RequestError)
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// List all threads
    ///
    /// # Arguments
    ///
    /// * `limit` - Optional result limit
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the request fails, authentication fails, or the server returns an error.
    pub async fn list_threads(&self, limit: Option<usize>) -> Result<Vec<Thread>, ClientError> {
        let url = format!("{}/threads", self.endpoint);
        let mut request = self.client.get(&url).apply_auth(&self.auth)?;

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(ClientError::RequestError)
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Delete thread
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn delete_thread(&self, thread_id: &str) -> Result<(), ClientError> {
        let url = format!("{}/threads/{}", self.endpoint, thread_id);
        let response = self
            .client
            .delete(&url)
            .apply_auth(&self.auth)?
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }
}

/// Extension trait for applying authentication to requests
trait RequestBuilderExt: Sized {
    fn apply_auth(self, auth: &AuthConfig) -> Result<Self, ClientError>;
}

impl RequestBuilderExt for reqwest::RequestBuilder {
    fn apply_auth(self, auth: &AuthConfig) -> Result<Self, ClientError> {
        match auth {
            AuthConfig::None => Ok(self),
            AuthConfig::Token(token) => Ok(self.bearer_auth(token)),
            AuthConfig::ApiKey { header, key } => Ok(self.header(header, key)),
        }
    }
}

/// Graph-specific client
///
/// Provides methods for invoking and managing a specific graph.
#[derive(Debug)]
pub struct GraphClient {
    /// HTTP client
    client: reqwest::Client,
    /// Graph endpoint
    endpoint: String,
    /// Authentication configuration
    auth: AuthConfig,
}

impl GraphClient {
    /// Create new `GraphClient`
    ///
    /// # Arguments
    ///
    /// * `client` - HTTP client
    /// * `endpoint` - Graph endpoint
    /// * `auth` - Authentication configuration
    #[must_use]
    pub(crate) const fn new(client: reqwest::Client, endpoint: String, auth: AuthConfig) -> Self {
        Self {
            client,
            endpoint,
            auth,
        }
    }

    /// Invoke graph synchronously
    ///
    /// # Type Parameters
    ///
    /// * `S` - Output state type
    ///
    /// # Arguments
    ///
    /// * `input` - Input state as JSON
    /// * `config` - Optional invocation configuration
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the request fails, authentication fails, or the server returns an error.
    pub async fn invoke<S: for<'de> Deserialize<'de>>(
        &self,
        input: serde_json::Value,
        config: Option<InvokeConfig>,
    ) -> Result<S, ClientError> {
        let response = self
            .client
            .post(format!("{}/invoke", self.endpoint))
            .apply_auth(&self.auth)?
            .json(&serde_json::json!({
                "input": input,
                "config": config
            }))
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response.json().await.map_err(ClientError::RequestError)
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get current state
    ///
    /// # Type Parameters
    ///
    /// * `T` - State type
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn get_state<T: for<'de> Deserialize<'de>>(
        &self,
        thread_id: &str,
    ) -> Result<StateSnapshot<T>, ClientError> {
        let response = self
            .client
            .get(format!("{}/threads/{}/state", self.endpoint, thread_id))
            .apply_auth(&self.auth)?
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response.json().await.map_err(ClientError::RequestError)
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get state history
    ///
    /// # Type Parameters
    ///
    /// * `T` - State type
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    /// * `limit` - Optional result limit
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn get_state_history<T: for<'de> Deserialize<'de>>(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<StateSnapshot<T>>, ClientError> {
        let url = format!("{}/threads/{}/history", self.endpoint, thread_id);
        let mut request = self.client.get(&url).apply_auth(&self.auth)?;

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)]);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response.json().await.map_err(ClientError::RequestError)
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Update state
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    /// * `update` - State update as JSON
    /// * `as_node` - Optional node name for update
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn update_state(
        &self,
        thread_id: &str,
        update: serde_json::Value,
        as_node: Option<&str>,
    ) -> Result<(), ClientError> {
        let response = self
            .client
            .post(format!("{}/threads/{}/state", self.endpoint, thread_id))
            .apply_auth(&self.auth)?
            .json(&serde_json::json!({
                "update": update,
                "as_node": as_node
            }))
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Resume execution
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    /// * `values` - Resume values
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the thread is not found, the request fails, or the server returns an error.
    pub async fn resume(
        &self,
        thread_id: &str,
        values: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let response = self
            .client
            .post(format!("{}/threads/{}/resume", self.endpoint, thread_id))
            .apply_auth(&self.auth)?
            .json(&serde_json::json!({ "values": values }))
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            response.json().await.map_err(ClientError::RequestError)
        } else if response.status() == 404 {
            Err(ClientError::ThreadNotFound(thread_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Cancel execution
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier
    /// * `run_id` - Run identifier
    ///
    /// # Errors
    ///
    /// Returns `ClientError` if the run is not found, the request fails, or the server returns an error.
    pub async fn cancel(&self, thread_id: &str, run_id: &str) -> Result<(), ClientError> {
        let response = self
            .client
            .post(format!(
                "{}/threads/{}/runs/{}/cancel",
                self.endpoint, thread_id, run_id
            ))
            .apply_auth(&self.auth)?
            .send()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else if response.status() == 404 {
            Err(ClientError::RunNotFound(run_id.to_string()))
        } else {
            Err(ClientError::Server {
                status: response.status().as_u16(),
                message: response.text().await.unwrap_or_default(),
            })
        }
    }

    /// Get the HTTP client
    #[must_use]
    pub(crate) const fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Get the endpoint
    #[must_use]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Get the auth configuration
    #[must_use]
    pub(crate) const fn auth(&self) -> &AuthConfig {
        &self.auth
    }
}

// Rust guideline compliant 2026-05-19
