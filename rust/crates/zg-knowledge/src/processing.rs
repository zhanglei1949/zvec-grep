//! Graph-processing budgets and stable persisted fingerprints. Scheduling is owned by the engine.
use crate::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, time::Instant};

pub fn stable_digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(zg_storage::sha256_hex(canonical_json(value)?.as_bytes()))
}

/// Sorted object keys, preserved array order and normal JSON escaping.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<String> {
    fn canonical(value: &Value, output: &mut String) -> Result<()> {
        match value {
            Value::Object(map) => {
                output.push('{');
                for (i, (key, value)) in map.iter().collect::<BTreeMap<_, _>>().iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .map_err(|_| Error::InvalidInput("invalid fingerprint"))?,
                    );
                    output.push(':');
                    canonical(value, output)?;
                }
                output.push('}');
            }
            Value::Array(items) => {
                output.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    canonical(item, output)?;
                }
                output.push(']');
            }
            _ => output.push_str(
                &serde_json::to_string(value)
                    .map_err(|_| Error::InvalidInput("invalid fingerprint"))?,
            ),
        }
        Ok(())
    }
    let value =
        serde_json::to_value(value).map_err(|_| Error::InvalidInput("invalid fingerprint"))?;
    let mut encoded = String::new();
    canonical(&value, &mut encoded)?;
    Ok(encoded)
}

#[derive(Clone, Debug)]
pub struct ProcessingContext {
    pub max_input_bytes: usize,
    pub max_output_items: usize,
    pub deadline: Option<Instant>,
    pub cancellation: tokio_util::sync::CancellationToken,
}
impl ProcessingContext {
    pub fn check(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Error::Deadline);
        }
        Ok(())
    }
    pub fn check_input(&self, count: usize) -> Result<()> {
        self.check()?;
        if count > self.max_input_bytes {
            return Err(Error::ResourceExhausted);
        }
        Ok(())
    }
    pub fn check_output(&self, count: usize) -> Result<()> {
        self.check()?;
        if count > self.max_output_items {
            return Err(Error::ResourceExhausted);
        }
        Ok(())
    }
}
