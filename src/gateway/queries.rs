use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// A single query definition loaded from introspect.json.
#[derive(Clone, Debug, Deserialize)]
pub struct DbQuery {
    pub request_type: i32,
    pub parameters: pbjson_types::Struct,
    pub return_types: pbjson_types::Struct,
    pub is_embedding: bool,
    pub is_mcp: bool,
}

/// Container for query definitions loaded from introspect.json.
#[derive(Clone, Debug, Default)]
pub struct Queries {
    inner: Arc<HashMap<String, DbQuery>>,
}

impl Queries {
    pub fn from_map(map: HashMap<String, DbQuery>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }

    pub fn get(&self, name: impl AsRef<str>) -> Result<&DbQuery, ()> {
        self.inner.get(name.as_ref()).ok_or(())
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}
