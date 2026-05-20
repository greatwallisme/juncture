//! Checkpoint serialization
//!
//! Provides serialization abstractions and implementations for storing checkpoint data
//! in multiple formats (`MessagePack`, JSON, and optionally encrypted).

use crate::error::CheckpointError;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[cfg(feature = "encryption")]
use aes_gcm::{Aes256Gcm, Nonce, aead::Aead};

#[cfg(feature = "encryption")]
use aes_gcm::aead::{AeadCore, KeyInit, OsRng};

#[cfg(feature = "encryption")]
use pbkdf2::pbkdf2_hmac;

#[cfg(feature = "encryption")]
use sha2::Sha256;

/// Serialization format
///
/// Defines the supported serialization formats for checkpoint data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SerializationFormat {
    /// `MessagePack` binary format (default, high performance)
    #[default]
    MessagePack,

    /// JSON text format (human readable, debug friendly)
    Json,
}

/// Serializer kind for checkpoint data
///
/// An enum-dispatched serializer that can be stored in checkpoint savers without
/// requiring dynamic dispatch. Defaults to `MessagePack`.
#[derive(Clone, Debug, Default)]
pub enum SerializerKind {
    /// `MessagePack` binary format (default, high performance)
    #[default]
    MessagePack,
    /// JSON text format (human readable, debug friendly)
    Json,
}

impl SerializerKind {
    /// Serialize a serializable value to bytes using this serializer
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Serialize`] if serialization fails.
    pub fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError> {
        match self {
            Self::MessagePack => {
                rmp_serde::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
            }
            Self::Json => {
                serde_json::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
            }
        }
    }

    /// Deserialize bytes to a deserializable type using this serializer
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Deserialize`] if deserialization fails.
    pub fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        match self {
            Self::MessagePack => {
                rmp_serde::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
            }
            Self::Json => serde_json::from_slice(data)
                .map_err(|e| CheckpointError::Deserialize(e.to_string())),
        }
    }

    /// Get the format identifier
    #[must_use]
    pub const fn format(&self) -> SerializationFormat {
        match self {
            Self::MessagePack => SerializationFormat::MessagePack,
            Self::Json => SerializationFormat::Json,
        }
    }
}

/// Checkpoint serializer trait
///
/// Abstraction over different serialization formats, allowing checkpoint storage
/// to use JSON, `MessagePack`, or custom serialization strategies.
pub trait CheckpointSerializer: Send + Sync + 'static {
    /// Serialize a JSON value to bytes
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Serialize`] if serialization fails.
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError>;

    /// Deserialize bytes back to a JSON value
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Deserialize`] if deserialization fails.
    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError>;

    /// Serialize any serializable type to bytes
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Serialize`] if serialization fails.
    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError>;

    /// Deserialize bytes to any deserializable type
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Deserialize`] if deserialization fails.
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError>;

    /// Get the format identifier
    #[must_use]
    fn format(&self) -> SerializationFormat;
}

/// `MessagePack` serializer
///
/// High-performance binary serialization using `MessagePack` format.
/// This is the default serializer for production use.
#[derive(Clone, Debug, Default)]
pub struct MsgpackSerializer;

impl MsgpackSerializer {
    /// Create a new `MessagePack` serializer
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CheckpointSerializer for MsgpackSerializer {
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError> {
        rmp_serde::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
    }

    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError> {
        rmp_serde::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError> {
        rmp_serde::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        rmp_serde::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::MessagePack
    }
}

/// JSON serializer
///
/// Human-readable text serialization using JSON format.
/// Useful for debugging and development environments.
#[derive(Clone, Debug, Default)]
pub struct JsonSerializer;

impl JsonSerializer {
    /// Create a new JSON serializer
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CheckpointSerializer for JsonSerializer {
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError> {
        serde_json::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
    }

    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError> {
        serde_json::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError> {
        serde_json::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        serde_json::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::Json
    }
}

/// JSON+ serializer (pretty-printed)
///
/// Like `JsonSerializer` but with pretty-printing for better human readability.
#[derive(Clone, Debug)]
pub struct JsonPlusSerializer {
    /// Pretty-print output
    pretty: bool,
}

impl JsonPlusSerializer {
    /// Create a new JSON+ serializer with pretty-printing
    #[must_use]
    pub const fn new() -> Self {
        Self { pretty: true }
    }

    /// Create a new JSON+ serializer with configurable pretty-printing
    #[must_use]
    pub const fn with_pretty(pretty: bool) -> Self {
        Self { pretty }
    }
}

impl Default for JsonPlusSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckpointSerializer for JsonPlusSerializer {
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError> {
        if self.pretty {
            serde_json::to_vec_pretty(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
        } else {
            serde_json::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
        }
    }

    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError> {
        serde_json::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError> {
        if self.pretty {
            serde_json::to_vec_pretty(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
        } else {
            serde_json::to_vec(value).map_err(|e| CheckpointError::Serialize(e.to_string()))
        }
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        serde_json::from_slice(data).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::Json
    }
}

/// Encrypted serializer wrapper
///
/// Wraps any inner serializer with AES-256-GCM encryption for secure storage.
///
/// # Feature
///
/// Only available when the `encryption` feature is enabled.
#[cfg(feature = "encryption")]
#[derive(Clone, Debug)]
pub struct EncryptedSerializer<S: CheckpointSerializer> {
    /// Inner serializer to use after encryption
    inner: S,
    /// AES-256-GCM key (32 bytes)
    key: [u8; 32],
}

#[cfg(feature = "encryption")]
impl<S: CheckpointSerializer> EncryptedSerializer<S> {
    /// Create a new encrypted serializer
    ///
    /// # Errors
    ///
    /// Returns error if the key is not exactly 32 bytes.
    ///
    /// # Panics
    ///
    /// Panics if key length is not 32 bytes (should never happen with proper validation).
    pub const fn new(inner: S, key: [u8; 32]) -> Self {
        Self { inner, key }
    }

    /// Create from a passphrase using PBKDF2
    ///
    /// Derives a 32-byte key from the provided passphrase using PBKDF2.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Serialize`] if key derivation fails.
    pub fn from_passphrase(
        inner: S,
        passphrase: &str,
        salt: &[u8; 32],
    ) -> Result<Self, CheckpointError> {
        let mut key = [0u8; 32];
        // pbkdf2_hmac doesn't return a Result, it computes in place
        pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, 100_000, &mut key);

        Ok(Self { inner, key })
    }
}

#[cfg(feature = "encryption")]
impl<S: CheckpointSerializer> CheckpointSerializer for EncryptedSerializer<S> {
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError> {
        // Serialize the value using inner serializer
        let plaintext = self.inner.serialize_value(value)?;

        // Generate random nonce
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| CheckpointError::Serialize(format!("Cipher init failed: {e}")))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

        // Encrypt
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|e| CheckpointError::Serialize(format!("Encryption failed: {e}")))?;

        // Format: nonce (12 bytes) + ciphertext
        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError> {
        if data.len() < 12 {
            return Err(CheckpointError::Deserialize(
                "Encrypted data too short".to_string(),
            ));
        }

        // Extract nonce and ciphertext
        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        // Decrypt
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| CheckpointError::Deserialize(format!("Cipher init failed: {e}")))?;
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| CheckpointError::Deserialize(format!("Decryption failed: {e}")))?;

        // Deserialize using inner serializer
        self.inner.deserialize_value(&plaintext)
    }

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, CheckpointError> {
        // Convert to JSON value first
        let json_value =
            serde_json::to_value(value).map_err(|e| CheckpointError::Serialize(e.to_string()))?;
        self.serialize_value(&json_value)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        let json_value = self.deserialize_value(data)?;
        serde_json::from_value(json_value).map_err(|e| CheckpointError::Deserialize(e.to_string()))
    }

    fn format(&self) -> SerializationFormat {
        self.inner.format()
    }
}

/// Detect serialization format from raw bytes
///
/// Examines the byte sequence to determine if it's `MessagePack` or JSON format.
///
/// # Examples
///
/// ```
/// use juncture_checkpoint::serde::{detect_format, SerializationFormat};
///
/// let json_data = b"{\"key\":\"value\"}";
/// let format = detect_format(json_data);
/// assert_eq!(format, SerializationFormat::Json);
/// ```
#[must_use]
pub fn detect_format(data: &[u8]) -> SerializationFormat {
    // MessagePack format detection
    // Common MessagePack markers: 0x82 (fixmap), 0x83 (fixmap), 0xde (map16)
    // JSON format: starts with '{' (0x7b) or '[' (0x5b) or whitespace
    if data.is_empty() {
        return SerializationFormat::Json;
    }

    let first_byte = data[0];

    // JSON format
    if first_byte == b'{' || first_byte == b'[' || first_byte.is_ascii_whitespace() {
        return SerializationFormat::Json;
    }

    // MessagePack format detection (heuristic)
    // fixmap: 0x80-0x8f, fixarray: 0x90-0x9f, map16: 0xde, map32: 0xdf
    // array16: 0xdc, array32: 0xdd
    if (0x80..=0x9f).contains(&first_byte)
        || first_byte == 0xde
        || first_byte == 0xdf
        || first_byte == 0xdc
        || first_byte == 0xdd
    {
        return SerializationFormat::MessagePack;
    }

    // Default to JSON for unknown formats
    SerializationFormat::Json
}

/// Deserialize bytes using format auto-detection
///
/// Detects whether the data is `MessagePack` or JSON, then deserializes
/// using the appropriate serializer. Falls back to JSON deserialization
/// if detection is ambiguous.
///
/// This function provides backwards compatibility when reading checkpoints
/// that were written with a different serializer (e.g., old JSON data
/// read by a saver now defaulting to `MessagePack`).
///
/// # Errors
///
/// Returns [`CheckpointError::Deserialize`] if neither `MessagePack` nor JSON
/// deserialization succeeds.
pub fn deserialize_auto<T: DeserializeOwned>(data: &[u8]) -> Result<T, CheckpointError> {
    let format = detect_format(data);
    match format {
        SerializationFormat::MessagePack => {
            // Try msgpack first, fall back to JSON if detection was wrong
            MsgpackSerializer::new()
                .deserialize::<T>(data)
                .or_else(|_| JsonSerializer::new().deserialize::<T>(data))
        }
        SerializationFormat::Json => JsonSerializer::new().deserialize::<T>(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_msgpack_serializer_roundtrip() {
        let ser = MsgpackSerializer::new();
        let original = json!({"key": "value", "number": 42});

        let serialized_data = ser.serialize_value(&original).unwrap();
        let deserialized = ser.deserialize_value(&serialized_data).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_json_serializer_roundtrip() {
        let ser = JsonSerializer::new();
        let original = json!({"key": "value", "number": 42});

        let serialized_data = ser.serialize_value(&original).unwrap();
        let deserialized = ser.deserialize_value(&serialized_data).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_json_plus_serializer_pretty() {
        let ser = JsonPlusSerializer::new();
        let original = json!({"key": "value", "nested": {"a": 1}});

        let serialized_data = ser.serialize_value(&original).unwrap();
        let serialized_str = std::str::from_utf8(&serialized_data).unwrap();

        // Pretty-printed should contain newlines/indentation
        assert!(serialized_str.contains('\n'));

        let deserialized = ser.deserialize_value(&serialized_data).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_checkpoint_detect_format_json() {
        let json_data = b"{\"key\":\"value\"}";
        let format = detect_format(json_data);
        assert_eq!(format, SerializationFormat::Json);
    }

    #[test]
    fn test_checkpoint_detect_format_msgpack() {
        // Create actual MessagePack data
        let serializer = MsgpackSerializer::new();
        let value = json!({"key": "value"});
        let msgpack_data = serializer.serialize_value(&value).unwrap();

        let format = detect_format(&msgpack_data);
        assert_eq!(format, SerializationFormat::MessagePack);
    }

    #[test]
    fn test_checkpoint_detect_format_empty() {
        let format = detect_format(&[]);
        assert_eq!(format, SerializationFormat::Json);
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn test_encrypted_serializer() {
        use aes_gcm::aead::rand_core::RngCore;

        let inner = JsonSerializer::new();
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);

        let serializer = EncryptedSerializer::new(inner, key);
        let original = json!({"secret": "data"});

        let encrypted = serializer.serialize_value(&original).unwrap();

        // Encrypted data should be larger (nonce + ciphertext)
        assert!(encrypted.len() > original.to_string().len());

        let decrypted = serializer.deserialize_value(&encrypted).unwrap();
        assert_eq!(original, decrypted);
    }

    #[test]
    fn test_serialization_format_eq() {
        assert_eq!(
            SerializationFormat::MessagePack,
            SerializationFormat::MessagePack
        );
        assert_eq!(SerializationFormat::Json, SerializationFormat::Json);
        assert_ne!(SerializationFormat::MessagePack, SerializationFormat::Json);
    }
}

// Rust guideline compliant 2026-05-20
