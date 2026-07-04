use std::sync::Arc;

use anyhow::Result;
use arrow::array::{Array, FixedSizeListArray, Float32Array, Float64Array, Int32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use futures_util::TryStreamExt;
use lancedb::connect;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde::{Deserialize, Serialize};

/// Knowledge base index backed by LanceDB.
#[derive(Debug, Clone)]
pub struct KnowledgeBase {
    pub uri: String,
    pub dim: usize,
}

/// A search result from the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub doc_id: String,
    pub text: String,
    #[serde(default)]
    pub metadata: String,
    pub score: f64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_doc_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i32>,
}

/// A search result with neighboring chunk context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSearchResult {
    pub doc_id: String,
    pub text: String,
    #[serde(default)]
    pub metadata: String,
    pub score: f64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_doc_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i32>,
    #[serde(default)]
    pub context_before: Vec<String>,
    #[serde(default)]
    pub context_after: Vec<String>,
}

/// Sentinel chunk_index for full documents (not chunks).
const CHUNK_INDEX_FULL_DOC: i32 = -1;

fn kb_schema(dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("parent_doc_id", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int32, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim as i32,
            ),
            false,
        ),
    ]))
}

fn embedding_array(dim: usize, values: &[f32]) -> FixedSizeListArray {
    FixedSizeListArray::new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        dim as i32,
        Arc::new(Float32Array::from(values.to_vec())),
        None,
    )
}

impl KnowledgeBase {
    pub fn new(uri: String, dim: usize) -> Self {
        Self { uri, dim }
    }

    async fn conn(&self) -> Result<lancedb::connection::Connection> {
        Ok(connect(&self.uri).execute().await?)
    }

    async fn ensure_table(&self, conn: &lancedb::connection::Connection, namespace: &str) -> Result<()> {
        if conn.open_table(namespace).execute().await.is_ok() {
            return Ok(());
        }
        conn.create_table(namespace, RecordBatch::new_empty(kb_schema(self.dim)))
            .execute()
            .await?;
        Ok(())
    }

    /// Index a single document or chunk.
    ///
    /// For a full document, pass `parent_doc_id` = `doc_id` and `chunk_index` = -1.
    /// For a chunk, pass the parent doc_id and the 0-based chunk index.
    pub async fn index(
        &self,
        namespace: &str,
        doc_id: &str,
        text: &str,
        embedding: &[f32],
        metadata: Option<&str>,
        parent_doc_id: &str,
        chunk_index: i32,
    ) -> Result<()> {
        let conn = self.conn().await?;
        self.ensure_table(&conn, namespace).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let batch = RecordBatch::try_new(
            kb_schema(self.dim),
            vec![
                Arc::new(StringArray::from(vec![doc_id])),
                Arc::new(StringArray::from(vec![text])),
                Arc::new(StringArray::from(vec![metadata.unwrap_or("{}")])),
                Arc::new(StringArray::from(vec![now.as_str()])),
                Arc::new(StringArray::from(vec![parent_doc_id])),
                Arc::new(Int32Array::from(vec![chunk_index])),
                Arc::new(embedding_array(self.dim, embedding)),
            ],
        )?;

        let table = conn.open_table(namespace).execute().await?;
        table.add(batch).execute().await?;
        Ok(())
    }

    /// Search for the `k` most similar documents.
    pub async fn search(
        &self,
        namespace: &str,
        query_embedding: &[f32],
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        let conn = self.conn().await?;
        let table = conn.open_table(namespace).execute().await?;

        let results = table
            .vector_search(query_embedding)?
            .limit(k)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let mut docs = Vec::new();

        for batch in &batches {
            let doc_ids = str_col(batch, "doc_id");
            let texts = str_col(batch, "text");
            let metadatas = str_col(batch, "metadata");
            let created_ats = str_col(batch, "created_at");
            let parents = str_col(batch, "parent_doc_id");
            let chunk_idxs = i32_col(batch, "chunk_index");
            let scores = distance_col(batch);

            for i in 0..batch.num_rows() {
                let pdi = parents.value(i);
                let ci = chunk_idxs.value(i);
                docs.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    text: texts.value(i).to_string(),
                    metadata: metadatas.value(i).to_string(),
                    score: scores[i] as f64,
                    created_at: created_ats.value(i).to_string(),
                    parent_doc_id: if ci == CHUNK_INDEX_FULL_DOC { None } else { Some(pdi.to_string()) },
                    chunk_index: if ci == CHUNK_INDEX_FULL_DOC { None } else { Some(ci) },
                });
            }
        }

        Ok(docs)
    }

    /// Search with context: returns matching chunks with N neighboring chunks.
    pub async fn search_with_context(
        &self,
        namespace: &str,
        query_embedding: &[f32],
        k: usize,
        context: usize,
    ) -> Result<Vec<ContextSearchResult>> {
        let results = self.search(namespace, query_embedding, k).await?;
        let mut ctx_results = Vec::new();

        for r in &results {
            let parent = r.parent_doc_id.clone().unwrap_or_else(|| r.doc_id.clone());
            let ctx = self
                .get_chunk_context(namespace, &parent, r.chunk_index, context)
                .await
                .unwrap_or_default();

            ctx_results.push(ContextSearchResult {
                doc_id: r.doc_id.clone(),
                text: r.text.clone(),
                metadata: r.metadata.clone(),
                score: r.score,
                created_at: r.created_at.clone(),
                parent_doc_id: r.parent_doc_id.clone(),
                chunk_index: r.chunk_index,
                context_before: ctx.0,
                context_after: ctx.1,
            });
        }

        Ok(ctx_results)
    }

    /// Get neighboring chunks for a parent document.
    async fn get_chunk_context(
        &self,
        namespace: &str,
        parent_doc_id: &str,
        chunk_index: Option<i32>,
        context: usize,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let conn = self.conn().await?;
        let table = conn.open_table(namespace).execute().await?;

        let dummy: Vec<f32> = vec![0.0; self.dim];
        let stream = table
            .vector_search(dummy.as_slice())?
            .limit(u32::MAX.try_into().unwrap())
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut chunks: Vec<(i32, String)> = Vec::new();

        for batch in &batches {
            let parents = str_col(batch, "parent_doc_id");
            let texts = str_col(batch, "text");
            let idxs = i32_col(batch, "chunk_index");

            for i in 0..batch.num_rows() {
                if parents.value(i) == parent_doc_id && idxs.value(i) != CHUNK_INDEX_FULL_DOC {
                    chunks.push((idxs.value(i), texts.value(i).to_string()));
                }
            }
        }

        chunks.sort_by_key(|(idx, _)| *idx);

        let Some(ci) = chunk_index else {
            return Ok((vec![], vec![]));
        };

        let Some(pos) = chunks.iter().position(|(idx, _)| *idx == ci) else {
            return Ok((vec![], vec![]));
        };

        let before: Vec<String> = chunks
            .iter()
            .take(pos)
            .rev()
            .take(context)
            .rev()
            .map(|(_, t)| t.clone())
            .collect();

        let after: Vec<String> = chunks
            .iter()
            .skip(pos + 1)
            .take(context)
            .map(|(_, t)| t.clone())
            .collect();

        Ok((before, after))
    }

    /// Delete a document by doc_id within a namespace.
    pub async fn delete(&self, namespace: &str, doc_id: &str) -> Result<()> {
        let conn = self.conn().await?;
        let table = conn.open_table(namespace).execute().await?;
        let escaped = doc_id.replace('\'', "''");
        let pred = format!("doc_id = '{escaped}'");
        table.delete(pred.as_str()).await?;
        Ok(())
    }

    /// List all documents in a namespace.
    pub async fn list(&self, namespace: &str) -> Result<Vec<SearchResult>> {
        let conn = self.conn().await?;
        let table = conn.open_table(namespace).execute().await?;

        let dummy: Vec<f32> = vec![0.0; self.dim];
        let results = table
            .vector_search(dummy.as_slice())?
            .limit(u32::MAX.try_into().unwrap())
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = results.try_collect().await?;
        let mut docs = Vec::new();

        for batch in &batches {
            let doc_ids = str_col(batch, "doc_id");
            let texts = str_col(batch, "text");
            let metadatas = str_col(batch, "metadata");
            let created_ats = str_col(batch, "created_at");
            let parents = str_col(batch, "parent_doc_id");
            let chunk_idxs = i32_col(batch, "chunk_index");

            for i in 0..batch.num_rows() {
                let pdi = parents.value(i);
                let ci = chunk_idxs.value(i);
                docs.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    text: texts.value(i).to_string(),
                    metadata: metadatas.value(i).to_string(),
                    score: 0.0,
                    created_at: created_ats.value(i).to_string(),
                    parent_doc_id: if ci == CHUNK_INDEX_FULL_DOC { None } else { Some(pdi.to_string()) },
                    chunk_index: if ci == CHUNK_INDEX_FULL_DOC { None } else { Some(ci) },
                });
            }
        }

        Ok(docs)
    }
}

/// Split text into overlapping chunks.
///
/// Splits on paragraph boundaries (double newline), then merges small paragraphs
/// until `chunk_size` chars are reached. If a single paragraph exceeds
/// `chunk_size`, it is hard-split.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }

    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for para in &paragraphs {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            continue;
        }

        // If this paragraph alone exceeds chunk_size, hard-split it
        if trimmed.len() > chunk_size {
            if !current.is_empty() {
                chunks.push(current.clone());
                current.clear();
            }
            let mut start = 0;
            while start < trimmed.len() {
                let end = (start + chunk_size).min(trimmed.len());
                let piece = trimmed[start..end].to_string();
                if !chunks.is_empty() && overlap > 0 {
                    let prev_end = start.saturating_sub(overlap);
                    let bridge = &trimmed[prev_end..start];
                    chunks.push(format!("{}...{}", bridge, piece));
                } else {
                    chunks.push(piece);
                }
                start = end.saturating_sub(overlap);
            }
            continue;
        }

        // If adding this paragraph would exceed chunk_size, flush current
        if !current.is_empty() && current.len() + trimmed.len() + 2 > chunk_size {
            if overlap > 0 && overlap < current.len() {
                let tail = current[current.len() - overlap..].to_string();
                chunks.push(current.clone());
                current = tail;
            } else {
                chunks.push(current.clone());
                current.clear();
            }
        }

        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(trimmed);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(text.to_string());
    }

    chunks
}

fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    let idx = batch
        .schema()
        .index_of(name)
        .unwrap_or_else(|_| panic!("column {name} not found"));
    batch.column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap_or_else(|| panic!("column {name} is not Utf8"))
}

fn i32_col<'a>(batch: &'a RecordBatch, name: &str) -> &'a Int32Array {
    let idx = batch
        .schema()
        .index_of(name)
        .unwrap_or_else(|_| panic!("column {name} not found"));
    batch.column(idx)
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap_or_else(|| panic!("column {name} is not Int32"))
}

fn distance_col(batch: &RecordBatch) -> Vec<f32> {
    let Ok(idx) = batch.schema().index_of("_distance") else {
        return vec![0.0; batch.num_rows()];
    };
    let col = batch.column(idx);
    if let Some(arr) = col.as_any().downcast_ref::<Float32Array>() {
        (0..batch.num_rows()).map(|i| arr.value(i)).collect()
    } else if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
        (0..batch.num_rows()).map(|i| arr.value(i) as f32).collect()
    } else {
        vec![0.0; batch.num_rows()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_no_split() {
        let text = "Sweden is a country.";
        let chunks = chunk_text(text, 512, 64);
        assert_eq!(chunks, vec![text]);
    }

    #[test]
    fn chunk_text_paragraph_split() {
        let text = "Paragraph one.\n\nParagraph two.\n\nParagraph three.";
        let chunks = chunk_text(text, 30, 0);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn chunk_text_hard_split_long() {
        let text = "A".repeat(200);
        let chunks = chunk_text(&text, 100, 0);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn chunk_text_empty() {
        assert!(chunk_text("", 512, 64).is_empty());
    }
}
