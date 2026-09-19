//! Cursor request-lifecycle resident-memory accounting.
//!
//! Cursor keeps substantially more request-owned state than a normal HTTP
//! adapter: decoded JSON, two plan generations, a bidirectional h2 parser,
//! response aggregation and (for tool calls) a parked session.  Estimates in
//! this module deliberately use allocation capacities where they are
//! available and saturating arithmetic throughout.  They are conservative
//! accounting estimates, not wire-size limits.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use bytes::Bytes;
use serde_json::Value;

use super::agent_proto::McpToolDef;
use super::image::ImageRef;
use super::request_builder::{
    AgentRunPlan, CompletedToolCall, ExtractedToolChoice, ResponseToolNamespace, ToolResultBlock,
};
use super::session::{CursorSession, PendingToolCall};
use crate::proxy::request_memory::{
    retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent, RequestMemoryError,
    RequestMemoryReservation,
};

fn string_bytes(value: &String) -> usize {
    size_of::<String>().saturating_add(value.capacity())
}

fn string_slice_bytes(values: &[String]) -> usize {
    values.iter().fold(
        values.len().saturating_mul(size_of::<String>()),
        |total, value| total.saturating_add(value.capacity()),
    )
}

fn value_slice_bytes(values: &[Value]) -> usize {
    values.iter().fold(
        values.len().saturating_mul(size_of::<Value>()),
        |total, value| total.saturating_add(retained_json_bytes(value)),
    )
}

fn tool_bytes(tool: &McpToolDef) -> usize {
    size_of::<McpToolDef>()
        .saturating_add(tool.name.capacity())
        .saturating_add(tool.description.capacity())
        .saturating_add(retained_json_bytes(tool.input_schema.as_json()))
        .saturating_add(tool.provider_identifier.capacity())
        .saturating_add(tool.tool_name.capacity())
}

fn tools_bytes(tools: &[McpToolDef]) -> usize {
    tools.iter().fold(
        tools.len().saturating_mul(size_of::<McpToolDef>()),
        |total, tool| total.saturating_add(tool_bytes(tool)),
    )
}

fn tool_result_bytes(result: &ToolResultBlock) -> usize {
    size_of::<ToolResultBlock>()
        .saturating_add(result.tool_call_id.capacity())
        .saturating_add(result.content.capacity())
}

fn tool_results_bytes(results: &[ToolResultBlock]) -> usize {
    results.iter().fold(
        results.len().saturating_mul(size_of::<ToolResultBlock>()),
        |total, result| total.saturating_add(tool_result_bytes(result)),
    )
}

fn completed_tool_call_bytes(call: &CompletedToolCall) -> usize {
    size_of::<CompletedToolCall>()
        .saturating_add(call.name.capacity())
        .saturating_add(retained_json_bytes(&call.arguments))
}

fn namespace_bytes(namespace: &ResponseToolNamespace) -> usize {
    size_of::<ResponseToolNamespace>()
        .saturating_add(namespace.internal_name.capacity())
        .saturating_add(namespace.namespace.capacity())
        .saturating_add(namespace.name.capacity())
}

fn image_ref_bytes(image: &ImageRef) -> usize {
    size_of::<ImageRef>().saturating_add(match image {
        ImageRef::DataUri(value) | ImageRef::HttpUrl(value) => value.capacity(),
        ImageRef::Inline { mime, data } => mime.capacity().saturating_add(data.len()),
    })
}

pub(crate) fn agent_run_plan_bytes(plan: &AgentRunPlan) -> usize {
    let system_prompt = plan
        .system_prompt
        .as_ref()
        .map(string_bytes)
        .unwrap_or_default();
    let namespaces = plan.response_tool_namespaces.iter().fold(
        plan.response_tool_namespaces
            .len()
            .saturating_mul(size_of::<ResponseToolNamespace>()),
        |total, namespace| total.saturating_add(namespace_bytes(namespace)),
    );
    let images = plan.images.iter().fold(
        plan.images.len().saturating_mul(size_of::<ImageRef>()),
        |total, image| total.saturating_add(image_ref_bytes(image)),
    );
    let completed_calls = plan.completed_tool_calls.iter().fold(
        plan.completed_tool_calls
            .len()
            .saturating_mul(size_of::<CompletedToolCall>()),
        |total, call| total.saturating_add(completed_tool_call_bytes(call)),
    );
    let named_choice = match &plan.tool_choice {
        ExtractedToolChoice::Named(name) => name.capacity(),
        _ => 0,
    };

    size_of::<AgentRunPlan>()
        .saturating_add(system_prompt)
        .saturating_add(plan.user_text.capacity())
        .saturating_add(tools_bytes(&plan.tools))
        .saturating_add(string_slice_bytes(&plan.custom_tool_names))
        .saturating_add(namespaces)
        .saturating_add(images)
        .saturating_add(tool_results_bytes(&plan.historical_tool_results))
        .saturating_add(tool_results_bytes(&plan.tool_results))
        .saturating_add(completed_calls)
        .saturating_add(plan.model_id.capacity())
        .saturating_add(
            plan.previous_response_id
                .as_ref()
                .map(String::capacity)
                .unwrap_or_default(),
        )
        .saturating_add(plan.working_directory.capacity())
        .saturating_add(named_choice)
        .saturating_add(value_slice_bytes(&plan.response_input_items))
}

pub(crate) fn buffered_events_bytes(events: &[String]) -> usize {
    events.iter().fold(
        events.len().saturating_mul(size_of::<String>()),
        |total, event| total.saturating_add(event.capacity()),
    )
}

pub(crate) fn string_set_bytes(values: &HashSet<String>) -> usize {
    values.iter().fold(
        values
            .capacity()
            .saturating_mul(size_of::<String>().saturating_add(size_of::<usize>())),
        |total, value| total.saturating_add(value.capacity()),
    )
}

fn pending_tool_call_bytes(call_id: &String, call: &PendingToolCall) -> usize {
    size_of::<String>()
        .saturating_add(size_of::<PendingToolCall>())
        .saturating_add(call_id.capacity())
        .saturating_add(call.exec_id.capacity())
        .saturating_add(call.tool_name.capacity())
}

fn pending_tool_calls_bytes(values: &HashMap<String, PendingToolCall>) -> usize {
    values.iter().fold(
        values.capacity().saturating_mul(
            size_of::<String>()
                .saturating_add(size_of::<PendingToolCall>())
                .saturating_add(size_of::<usize>()),
        ),
        |total, (id, call)| total.saturating_add(pending_tool_call_bytes(id, call)),
    )
}

pub(crate) fn session_retained_bytes(session: &CursorSession) -> usize {
    let namespaces = session.response_tool_namespaces.iter().fold(
        session
            .response_tool_namespaces
            .len()
            .saturating_mul(size_of::<ResponseToolNamespace>()),
        |total, namespace| total.saturating_add(namespace_bytes(namespace)),
    );
    let completed = session.cold_resume_completed_calls.iter().fold(
        session
            .cold_resume_completed_calls
            .len()
            .saturating_mul(size_of::<(String, Value)>()),
        |total, (name, arguments)| {
            total
                .saturating_add(name.capacity())
                .saturating_add(retained_json_bytes(arguments))
        },
    );
    let blobs = session.blob_store.iter().fold(
        session.blob_store.capacity().saturating_mul(
            size_of::<String>()
                .saturating_add(size_of::<Bytes>())
                .saturating_add(size_of::<usize>()),
        ),
        |total, (key, value)| {
            total
                .saturating_add(key.capacity())
                .saturating_add(value.len())
        },
    );

    size_of::<CursorSession>()
        .saturating_add(string_slice_bytes(&session.declared_tool_names))
        .saturating_add(tools_bytes(&session.declared_tools))
        .saturating_add(string_slice_bytes(&session.custom_tool_names))
        .saturating_add(namespaces)
        .saturating_add(value_slice_bytes(&session.semantic_items))
        .saturating_add(session.local_task_state.origin_turn_digest.capacity())
        .saturating_add(session.working_directory.capacity())
        .saturating_add(pending_tool_calls_bytes(&session.pending_tool_calls))
        .saturating_add(completed)
        .saturating_add(blobs)
}

pub(crate) struct CursorResponseMemory {
    budget: RequestMemoryBudget,
    retained: RequestMemoryReservation,
    buffered_events: RequestMemoryReservation,
}

impl CursorResponseMemory {
    pub(crate) fn new(budget: RequestMemoryBudget) -> Result<Self, RequestMemoryError> {
        Ok(Self {
            retained: budget.reserve(RequestMemoryComponent::StreamRetainedState, 0)?,
            buffered_events: budget.reserve(RequestMemoryComponent::NormalizedEvent, 0)?,
            budget,
        })
    }

    pub(crate) fn sync_retained(&self, bytes: usize) -> Result<(), RequestMemoryError> {
        self.retained.resize(bytes)
    }

    pub(crate) fn sync_buffered_events(&self, events: &[String]) -> Result<(), RequestMemoryError> {
        self.buffered_events.resize(buffered_events_bytes(events))
    }

    pub(crate) fn reserve_transient(
        &self,
        bytes: usize,
    ) -> Result<RequestMemoryReservation, RequestMemoryError> {
        self.budget
            .reserve(RequestMemoryComponent::StreamRetainedState, bytes)
    }

    pub(crate) fn retain_body(&self, body: Bytes) -> Result<Bytes, RequestMemoryError> {
        self.budget
            .retain_bytes(RequestMemoryComponent::NormalizedEvent, body)
    }

    /// Bind an already-accounted buffered event to the shared batch
    /// reservation. The full batch remains charged until every emitted Bytes
    /// owner has been released by the downstream body.
    pub(crate) fn own_buffered_event(&self, event: String) -> Bytes {
        self.buffered_events
            .clone()
            .retain_bytes(Bytes::from(event))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::proxy::cursor::request_builder::{try_build_plan, InboundProtocol};

    #[test]
    fn plan_estimate_grows_with_prompt_tools_and_response_items() {
        let small = try_build_plan(
            InboundProtocol::OpenAiResponses,
            &json!({"model":"composer-2.5","input":"x"}),
        )
        .unwrap();
        let large = try_build_plan(
            InboundProtocol::OpenAiResponses,
            &json!({
                "model":"composer-2.5",
                "input":[{"role":"user","content":[{"type":"input_text","text":"x".repeat(4096)}]}],
                "tools":[{"type":"function","name":"lookup","description":"d".repeat(2048),"parameters":{"type":"object","properties":{"query":{"type":"string"}}}}]
            }),
        )
        .unwrap();
        assert!(agent_run_plan_bytes(&large) > agent_run_plan_bytes(&small) + 4_096);
    }

    #[test]
    fn buffered_event_estimate_uses_string_capacity() {
        let mut event = String::with_capacity(4096);
        event.push_str("data: fixture\n\n");
        assert!(buffered_events_bytes(&[event]) >= 4096);
    }
}
