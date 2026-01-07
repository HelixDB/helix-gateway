//! MCP (Model Context Protocol) connection state management.
//!
//! Provides Redis-backed storage for MCP connection state, enabling
//! stateless gateway nodes to accumulate query steps across requests.

use crate::config::RedisConfig;
use crate::error::GatewayError;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

/// Redis-backed MCP connection state manager.
#[derive(Clone)]
pub struct McpStateManager {
    conn: ConnectionManager,
    ttl_secs: u64,
}

impl McpStateManager {
    /// Creates a new state manager connected to Redis.
    pub async fn new(config: &RedisConfig) -> Result<Self, GatewayError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| GatewayError::RedisError(format!("Redis client error: {}", e)))?;

        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| GatewayError::RedisError(format!("Redis connection error: {}", e)))?;

        Ok(Self {
            conn,
            ttl_secs: config.mcp_ttl_secs,
        })
    }

    fn key(connection_id: &str) -> String {
        format!("mcp:{}", connection_id)
    }

    /// Retrieves connection state from Redis, or creates a new one if not found.
    pub async fn get_or_create(&self, connection_id: &str) -> Result<McpConnectionState, GatewayError> {
        let mut conn = self.conn.clone();
        let key = Self::key(connection_id);

        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| GatewayError::RedisError(format!("Redis get error: {}", e)))?;

        match value {
            Some(json) => {
                let state: McpConnectionState = serde_json::from_str(&json)
                    .map_err(|e| GatewayError::RedisError(format!("JSON parse error: {}", e)))?;
                Ok(state)
            }
            None => Ok(McpConnectionState::new(connection_id.to_string())),
        }
    }

    /// Saves connection state to Redis with TTL refresh.
    pub async fn save_state(&self, state: &McpConnectionState) -> Result<(), GatewayError> {
        let mut conn = self.conn.clone();
        let key = Self::key(&state.connection_id);

        let json = serde_json::to_string(state)
            .map_err(|e| GatewayError::RedisError(format!("JSON serialize error: {}", e)))?;

        let _: () = conn
            .set_ex(&key, json, self.ttl_secs)
            .await
            .map_err(|e| GatewayError::RedisError(format!("Redis set error: {}", e)))?;

        Ok(())
    }
}

/// Persistent state for an MCP connection stored in Redis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConnectionState {
    pub connection_id: String,
    pub steps: Vec<ToolArgs>,
    pub current_position: usize,
}

impl McpConnectionState {
    pub fn new(connection_id: String) -> Self {
        Self {
            connection_id,
            steps: Vec::new(),
            current_position: 0,
        }
    }

    pub fn add_step(&mut self, step: ToolArgs) {
        self.steps.push(step);
        self.current_position = 0; // Reset position when chain changes
    }
}

// ============================================================================
// ToolArgs Definition (mirrored from helix-db for gateway serialization)
// ============================================================================

/// Query step types supported by MCP.
/// This mirrors the ToolArgs enum from helix-db for JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool_name", content = "args")]
#[serde(rename_all = "snake_case")]
pub enum ToolArgs {
    OutStep {
        edge_label: String,
        edge_type: EdgeType,
        filter: Option<FilterTraversal>,
    },
    OutEStep {
        edge_label: String,
        filter: Option<FilterTraversal>,
    },
    InStep {
        edge_label: String,
        edge_type: EdgeType,
        filter: Option<FilterTraversal>,
    },
    InEStep {
        edge_label: String,
        filter: Option<FilterTraversal>,
    },
    NFromType {
        node_type: String,
    },
    EFromType {
        edge_type: String,
    },
    FilterItems {
        #[serde(default)]
        filter: FilterTraversal,
    },
    OrderBy {
        properties: String,
        order: Order,
    },
    SearchKeyword {
        query: String,
        limit: usize,
        label: String,
    },
    SearchVecText {
        query: String,
        label: String,
        k: usize,
    },
    SearchVec {
        vector: Vec<f64>,
        k: usize,
        min_score: Option<f64>,
        cutoff: Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Node,
    Vec,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Order {
    #[serde(rename = "asc")]
    Asc,
    #[serde(rename = "desc")]
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilterTraversal {
    pub properties: Option<Vec<Vec<FilterProperties>>>,
    pub filter_traversals: Option<Vec<ToolArgs>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterProperties {
    pub key: String,
    pub value: serde_json::Value,
    pub operator: Option<Operator>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Operator {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Neq,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<=")]
    Lte,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_args_serialization() {
        let step = ToolArgs::NFromType {
            node_type: "person".to_string(),
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("n_from_type"));
        assert!(json.contains("person"));
    }

    #[test]
    fn test_tool_args_deserialization() {
        let json = r#"{"tool_name":"n_from_type","args":{"node_type":"person"}}"#;
        let step: ToolArgs = serde_json::from_str(json).unwrap();
        match step {
            ToolArgs::NFromType { node_type } => assert_eq!(node_type, "person"),
            _ => panic!("Expected NFromType"),
        }
    }

    #[test]
    fn test_connection_state_serialization() {
        let mut state = McpConnectionState::new("test-id".to_string());
        state.add_step(ToolArgs::NFromType {
            node_type: "user".to_string(),
        });

        let json = serde_json::to_string(&state).unwrap();
        let restored: McpConnectionState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.connection_id, "test-id");
        assert_eq!(restored.steps.len(), 1);
    }

    #[test]
    fn test_out_step_with_filter() {
        let json = r#"{
            "tool_name": "out_step",
            "args": {
                "edge_label": "knows",
                "edge_type": "node",
                "filter": {
                    "properties": [[{"key": "age", "value": 30, "operator": ">"}]]
                }
            }
        }"#;
        let step: ToolArgs = serde_json::from_str(json).unwrap();
        match step {
            ToolArgs::OutStep {
                edge_label,
                edge_type,
                filter,
            } => {
                assert_eq!(edge_label, "knows");
                assert!(matches!(edge_type, EdgeType::Node));
                assert!(filter.is_some());
            }
            _ => panic!("Expected OutStep"),
        }
    }
}
