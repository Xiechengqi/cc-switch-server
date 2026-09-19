use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub(crate) struct GroundingAccumulator {
    queries: Vec<String>,
    chunks: Vec<GroundingChunk>,
    raw_chunk_map: Vec<Option<usize>>,
    supports: Vec<GroundingSupport>,
    support_keys: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct GroundingChunk {
    url: String,
    title: String,
}

#[derive(Debug, Clone)]
struct GroundingSupport {
    part_index: Option<usize>,
    start_byte: usize,
    end_byte: usize,
    raw_chunk_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextMapping {
    pub part_index: usize,
    pub block_index: u64,
    pub start_scalar_in_block: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroundingCitation {
    pub block_index: u64,
    pub start_index: usize,
    pub end_index: usize,
    pub url: String,
    pub title: String,
    pub cited_text: String,
}

impl GroundingCitation {
    pub(crate) fn anthropic(&self) -> Value {
        json!({
            "type": "web_search_result_location",
            "url": self.url,
            "title": self.title,
            "cited_text": self.cited_text
        })
    }

    pub(crate) fn responses_annotation(&self) -> Value {
        json!({
            "type": "url_citation",
            "url": self.url,
            "title": self.title,
            "start_index": self.start_index,
            "end_index": self.end_index,
            "text": self.cited_text
        })
    }

    pub(crate) fn chat_annotation(&self) -> Value {
        json!({
            "type": "url_citation",
            "url_citation": {
                "url": self.url,
                "title": self.title,
                "start_index": self.start_index,
                "end_index": self.end_index
            }
        })
    }
}

impl GroundingAccumulator {
    pub(crate) fn retained_bytes(&self) -> usize {
        let queries = self
            .queries
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(
                self.queries
                    .iter()
                    .map(|query| query.capacity())
                    .sum::<usize>(),
            );
        let chunks = self
            .chunks
            .capacity()
            .saturating_mul(std::mem::size_of::<GroundingChunk>())
            .saturating_add(
                self.chunks
                    .iter()
                    .map(|chunk| chunk.url.capacity().saturating_add(chunk.title.capacity()))
                    .sum::<usize>(),
            );
        let raw_chunk_map = self
            .raw_chunk_map
            .capacity()
            .saturating_mul(std::mem::size_of::<Option<usize>>());
        let supports = self
            .supports
            .capacity()
            .saturating_mul(std::mem::size_of::<GroundingSupport>())
            .saturating_add(
                self.supports
                    .iter()
                    .map(|support| {
                        support
                            .raw_chunk_indices
                            .capacity()
                            .saturating_mul(std::mem::size_of::<usize>())
                    })
                    .sum::<usize>(),
            );
        let support_keys = self
            .support_keys
            .iter()
            .map(|key| {
                key.capacity()
                    .saturating_add(std::mem::size_of::<String>())
                    .saturating_add(std::mem::size_of::<usize>() * 3)
            })
            .sum::<usize>();
        queries
            .saturating_add(chunks)
            .saturating_add(raw_chunk_map)
            .saturating_add(supports)
            .saturating_add(support_keys)
    }

    pub(crate) fn from_candidate(candidate: &Value) -> Self {
        let mut grounding = Self::default();
        grounding.observe_candidate(candidate);
        grounding
    }

    pub(crate) fn observe_candidate(&mut self, candidate: &Value) {
        if let Some(metadata) = candidate
            .get("groundingMetadata")
            .or_else(|| candidate.get("grounding_metadata"))
        {
            self.observe_metadata(metadata);
        }
    }

    fn observe_metadata(&mut self, metadata: &Value) {
        if let Some(queries) = metadata
            .get("webSearchQueries")
            .or_else(|| metadata.get("web_search_queries"))
            .and_then(Value::as_array)
        {
            for query in queries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
            {
                if !self.queries.iter().any(|known| known == query) {
                    self.queries.push(query.to_string());
                }
            }
        }

        if let Some(chunks) = metadata
            .get("groundingChunks")
            .or_else(|| metadata.get("grounding_chunks"))
            .and_then(Value::as_array)
        {
            for chunk in chunks {
                let web = chunk.get("web").unwrap_or(chunk);
                let url = web
                    .get("uri")
                    .or_else(|| web.get("url"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                let title = web
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if url.is_empty() {
                    self.raw_chunk_map.push(None);
                    continue;
                }
                if let Some(index) = self.chunks.iter().position(|known| known.url == url) {
                    if self.chunks[index].title.is_empty() && !title.is_empty() {
                        self.chunks[index].title = title.to_string();
                    }
                    self.raw_chunk_map.push(Some(index));
                } else {
                    let index = self.chunks.len();
                    self.chunks.push(GroundingChunk {
                        url: url.to_string(),
                        title: title.to_string(),
                    });
                    self.raw_chunk_map.push(Some(index));
                }
            }
        }

        if let Some(supports) = metadata
            .get("groundingSupports")
            .or_else(|| metadata.get("grounding_supports"))
            .and_then(Value::as_array)
        {
            for support in supports {
                let segment = support.get("segment").unwrap_or(&Value::Null);
                let part_index = segment
                    .get("partIndex")
                    .or_else(|| segment.get("part_index"))
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok());
                let start_byte = segment
                    .get("startIndex")
                    .or_else(|| segment.get("start_index"))
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(0);
                let Some(end_byte) = segment
                    .get("endIndex")
                    .or_else(|| segment.get("end_index"))
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    continue;
                };
                if end_byte <= start_byte {
                    continue;
                }
                let raw_chunk_indices = support
                    .get("groundingChunkIndices")
                    .or_else(|| support.get("grounding_chunk_indices"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .filter_map(|value| usize::try_from(value).ok())
                    .collect::<Vec<_>>();
                if raw_chunk_indices.is_empty() {
                    continue;
                }
                let key = format!(
                    "{:?}:{start_byte}:{end_byte}:{raw_chunk_indices:?}",
                    part_index
                );
                if self.support_keys.insert(key) {
                    self.supports.push(GroundingSupport {
                        part_index,
                        start_byte,
                        end_byte,
                        raw_chunk_indices,
                    });
                }
            }
        }
    }

    pub(crate) fn has_web_grounding(&self) -> bool {
        !self.queries.is_empty() || !self.chunks.is_empty()
    }

    pub(crate) fn primary_query(&self) -> &str {
        self.queries.first().map(String::as_str).unwrap_or_default()
    }

    pub(crate) fn queries(&self) -> &[String] {
        &self.queries
    }

    pub(crate) fn response_sources(&self) -> Vec<Value> {
        self.chunks
            .iter()
            .map(|chunk| json!({"type": "url", "url": chunk.url, "title": chunk.title}))
            .collect()
    }

    pub(crate) fn anthropic_search_results(&self) -> Vec<Value> {
        self.chunks
            .iter()
            .map(|chunk| {
                json!({
                    "type": "web_search_result",
                    "url": chunk.url,
                    "title": chunk.title
                })
            })
            .collect()
    }

    pub(crate) fn citations(&self, mappings: &[TextMapping]) -> Vec<GroundingCitation> {
        let mut citations = Vec::new();
        let mut seen = BTreeSet::new();
        for support in &self.supports {
            let selected = mappings
                .iter()
                .filter(|mapping| {
                    support
                        .part_index
                        .is_none_or(|part_index| mapping.part_index == part_index)
                })
                .collect::<Vec<_>>();
            let ranges = mapped_ranges(&selected, support.start_byte, support.end_byte);
            if ranges.is_empty() {
                continue;
            }
            for raw_index in &support.raw_chunk_indices {
                let Some(chunk_index) = self.raw_chunk_map.get(*raw_index).copied().flatten()
                else {
                    continue;
                };
                let Some(chunk) = self.chunks.get(chunk_index) else {
                    continue;
                };
                for range in &ranges {
                    let key = format!(
                        "{}:{}:{}:{}",
                        range.block_index, chunk.url, range.start_index, range.end_index
                    );
                    if !seen.insert(key) {
                        continue;
                    }
                    citations.push(GroundingCitation {
                        block_index: range.block_index,
                        start_index: range.start_index,
                        end_index: range.end_index,
                        url: chunk.url.clone(),
                        title: chunk.title.clone(),
                        cited_text: range.cited_text.clone(),
                    });
                }
            }
        }
        citations
    }

    pub(crate) fn citations_by_block(
        &self,
        mappings: &[TextMapping],
    ) -> BTreeMap<u64, Vec<GroundingCitation>> {
        let mut by_block = BTreeMap::new();
        for citation in self.citations(mappings) {
            by_block
                .entry(citation.block_index)
                .or_insert_with(Vec::new)
                .push(citation);
        }
        by_block
    }
}

#[derive(Debug)]
struct MappedRange {
    block_index: u64,
    start_index: usize,
    end_index: usize,
    cited_text: String,
}

fn mapped_ranges(
    mappings: &[&TextMapping],
    mut start_byte: usize,
    mut end_byte: usize,
) -> Vec<MappedRange> {
    if mappings.is_empty() || start_byte >= end_byte {
        return Vec::new();
    }
    let total_bytes = mappings.iter().fold(0usize, |total, mapping| {
        total.saturating_add(mapping.text.len())
    });
    if start_byte >= total_bytes {
        return Vec::new();
    }
    end_byte = end_byte.min(total_bytes);
    start_byte = start_byte.min(end_byte);

    let mut ranges: Vec<MappedRange> = Vec::new();
    let mut cumulative = 0usize;
    for mapping in mappings {
        let mapping_start = cumulative;
        let mapping_end = mapping_start.saturating_add(mapping.text.len());
        cumulative = mapping_end;
        let overlap_start = start_byte.max(mapping_start);
        let overlap_end = end_byte.min(mapping_end);
        if overlap_start >= overlap_end {
            continue;
        }
        let relative_start = overlap_start - mapping_start;
        let relative_end = overlap_end - mapping_start;
        let start_scalar = scalar_offset(&mapping.text, relative_start);
        let end_scalar = scalar_offset(&mapping.text, relative_end);
        if end_scalar <= start_scalar {
            continue;
        }
        let absolute_start = mapping.start_scalar_in_block.saturating_add(start_scalar);
        let absolute_end = mapping.start_scalar_in_block.saturating_add(end_scalar);
        let cited_text = mapping
            .text
            .chars()
            .skip(start_scalar)
            .take(end_scalar - start_scalar)
            .collect::<String>();
        if let Some(previous) = ranges.last_mut().filter(|previous| {
            previous.block_index == mapping.block_index && previous.end_index == absolute_start
        }) {
            previous.end_index = absolute_end;
            previous.cited_text.push_str(&cited_text);
        } else {
            ranges.push(MappedRange {
                block_index: mapping.block_index,
                start_index: absolute_start,
                end_index: absolute_end,
                cited_text,
            });
        }
    }
    ranges
}

pub(crate) fn scalar_offset(text: &str, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(text.len());
    text.char_indices()
        .take_while(|(index, _)| *index < byte_offset)
        .count()
}

pub(crate) fn scalar_range_for_cited_text(
    text: &str,
    cited_text: &str,
    search_from_byte: usize,
) -> Option<(usize, usize, usize)> {
    if cited_text.is_empty() || search_from_byte > text.len() {
        return None;
    }
    let relative = text.get(search_from_byte..)?.find(cited_text)?;
    let start_byte = search_from_byte.saturating_add(relative);
    let end_byte = start_byte.saturating_add(cited_text.len());
    Some((
        scalar_offset(text, start_byte),
        scalar_offset(text, end_byte),
        end_byte,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_use_unicode_scalar_offsets_and_ignore_bad_or_duplicate_chunks() {
        let candidate = json!({
            "groundingMetadata": {
                "webSearchQueries": ["rust unicode", "rust unicode"],
                "groundingChunks": [
                    {"web": {"uri": "https://example.test/a", "title": "A"}},
                    {"web": {"uri": "https://example.test/a", "title": "duplicate"}},
                    {"web": {"uri": "https://example.test/b", "title": "B"}}
                ],
                "groundingSupports": [{
                    "segment": {"partIndex": 0, "startIndex": 1, "endIndex": 11},
                    "groundingChunkIndices": [0, 1, 2, 99]
                }]
            }
        });
        let grounding = GroundingAccumulator::from_candidate(&candidate);
        let citations = grounding.citations(&[TextMapping {
            part_index: 0,
            block_index: 0,
            start_scalar_in_block: 0,
            text: "A中🙂e\u{301}Z".to_string(),
        }]);
        assert_eq!(grounding.queries(), &["rust unicode"]);
        assert_eq!(grounding.response_sources().len(), 2);
        assert_eq!(citations.len(), 2);
        assert_eq!((citations[0].start_index, citations[0].end_index), (1, 5));
        assert_eq!(citations[0].cited_text, "中🙂e\u{301}");
        assert_eq!(citations[1].url, "https://example.test/b");
    }

    #[test]
    fn citation_ranges_can_span_incremental_mappings() {
        let candidate = json!({
            "groundingMetadata": {
                "groundingChunks": [{"web": {"uri": "https://example.test"}}],
                "groundingSupports": [{
                    "segment": {"partIndex": 0, "startIndex": 1, "endIndex": 5},
                    "groundingChunkIndices": [0]
                }]
            }
        });
        let grounding = GroundingAccumulator::from_candidate(&candidate);
        let citations = grounding.citations(&[
            TextMapping {
                part_index: 0,
                block_index: 7,
                start_scalar_in_block: 0,
                text: "ab".to_string(),
            },
            TextMapping {
                part_index: 0,
                block_index: 7,
                start_scalar_in_block: 2,
                text: "中c".to_string(),
            },
        ]);
        assert_eq!(citations.len(), 1);
        assert_eq!((citations[0].start_index, citations[0].end_index), (1, 3));
        assert_eq!(citations[0].cited_text, "b中");
    }
}
