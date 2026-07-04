use std::sync::Arc;

use anyhow::Result;
use arrow::array::{Array, FixedSizeListArray, Float32Array, Float64Array, StringArray};
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
}

fn kb_schema(dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, true),
        Field::new("created_at", DataType::Utf8, false),
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
        let batch = RecordBatch::new_empty(kb_schema(self.dim));
        conn.create_table(namespace, batch).execute().await?;
        Ok(())
    }

    /// Index a document: embed, store, and optionally associate metadata.
    pub async fn index(
        &self,
        namespace: &str,
        doc_id: &str,
        text: &str,
        embedding: &[f32],
        metadata: Option<&str>,
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
            let scores = distance_col(batch);

            for i in 0..batch.num_rows() {
                docs.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    text: texts.value(i).to_string(),
                    metadata: metadatas.value(i).to_string(),
                    score: scores[i] as f64,
                    created_at: created_ats.value(i).to_string(),
                });
            }
        }

        Ok(docs)
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

            for i in 0..batch.num_rows() {
                docs.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    text: texts.value(i).to_string(),
                    metadata: metadatas.value(i).to_string(),
                    score: 0.0,
                    created_at: created_ats.value(i).to_string(),
                });
            }
        }

        Ok(docs)
    }
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
