use bytes::Bytes;
use serde_json::Value;

use super::{RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation};
use crate::proxy::ProxyError;

pub(super) fn reserve(
    budget: Option<&RequestMemoryBudget>,
    component: RequestMemoryComponent,
    bytes: usize,
) -> Result<Option<RequestMemoryReservation>, ProxyError> {
    budget
        .map(|budget| {
            budget
                .reserve(component, bytes)
                .map_err(|e| e.into_proxy_error())
        })
        .transpose()
}

pub(super) fn resize(
    memory: &Option<RequestMemoryReservation>,
    bytes: usize,
) -> Result<(), ProxyError> {
    if let Some(memory) = memory {
        memory.resize(bytes).map_err(|e| e.into_proxy_error())?;
    }
    Ok(())
}

pub(super) fn serialize(
    value: &Value,
    budget: Option<&RequestMemoryBudget>,
) -> Result<Bytes, ProxyError> {
    let memory = reserve(
        budget,
        RequestMemoryComponent::NormalizedBody,
        super::serialized_json_bytes(value),
    )?;
    let bytes = Bytes::from(serde_json::to_vec(value)?);
    Ok(match memory {
        Some(memory) => memory.retain_bytes(bytes),
        None => bytes,
    })
}
