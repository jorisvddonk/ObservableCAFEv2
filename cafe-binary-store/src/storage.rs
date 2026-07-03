use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Manage binary files on disk with .writing sidecar for in-progress tracking.
pub struct Storage {
    data_dir: PathBuf,
}

impl Storage {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn chunk_path(&self, chunk_id: &str) -> PathBuf {
        let shard = &chunk_id[..2.min(chunk_id.len())];
        self.data_dir.join("binary").join(shard).join(chunk_id)
    }

    fn writing_path(&self, chunk_id: &str) -> PathBuf {
        let mut p = self.chunk_path(chunk_id);
        let name = format!("{}.writing", p.file_name().unwrap().to_string_lossy());
        p.set_file_name(name);
        p
    }

    /// Start a fresh write: create .writing marker. Returns 409 if already writing.
    pub async fn start_write(&self, chunk_id: &str) -> Result<()> {
        let cpath = self.chunk_path(chunk_id);
        let wpath = self.writing_path(chunk_id);
        if wpath.exists().await {
            anyhow::bail!("CONFLICT: write already in progress");
        }
        if let Some(parent) = cpath.parent() {
            fs::create_dir_all(parent)
                .await
                .context("failed to create shard dir")?;
        }
        // Create empty file + .writing marker
        fs::write(&cpath, &[])
            .await
            .context("failed to create chunk file")?;
        fs::write(&wpath, b"1")
            .await
            .context("failed to create .writing marker")?;
        Ok(())
    }

    /// Resume: verify .writing exists and file is at least offset bytes.
    pub async fn resume_write(&self, chunk_id: &str, offset: u64) -> Result<()> {
        let wpath = self.writing_path(chunk_id);
        if !wpath.exists().await {
            anyhow::bail!("NOT_FOUND: no in-progress write to resume");
        }
        let cpath = self.chunk_path(chunk_id);
        let meta = fs::metadata(&cpath).await.context("file not found")?;
        if meta.len() < offset {
            anyhow::bail!(
                "INVALID_OFFSET: file is {} bytes, requested offset {}",
                meta.len(),
                offset
            );
        }
        Ok(())
    }

    /// Append bytes to the chunk file at the given offset.
    pub async fn append(
        &self,
        chunk_id: &str,
        offset: u64,
        data: &[u8],
        max_bytes: u64,
    ) -> Result<()> {
        let cpath = self.chunk_path(chunk_id);
        let _meta = fs::metadata(&cpath).await.context("file not found")?;
        let new_len = offset as u64 + data.len() as u64;
        if new_len > max_bytes {
            anyhow::bail!(
                "PAYLOAD_TOO_LARGE: max {} bytes, would be {}",
                max_bytes,
                new_len
            );
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&cpath)
            .await
            .context("failed to open file")?;
        use tokio::io::SeekFrom;
        tokio::io::AsyncSeekExt::seek(&mut file, SeekFrom::Start(offset))
            .await
            .context("failed to seek")?;
        file.write_all(data)
            .await
            .context("failed to write data")?;
        Ok(())
    }

    /// Finalize: remove .writing marker.
    pub async fn finalize(&self, chunk_id: &str) -> Result<()> {
        let wpath = self.writing_path(chunk_id);
        let _ = fs::remove_file(&wpath).await;
        Ok(())
    }

    /// Read bytes from the file, starting at offset, up to max_len bytes.
    pub async fn read(
        &self,
        chunk_id: &str,
        offset: u64,
        max_len: u64,
    ) -> Result<(Vec<u8>, u64, bool)> {
        let cpath = self.chunk_path(chunk_id);
        let meta = fs::metadata(&cpath).await.context("file not found")?;
        let file_size = meta.len();
        let is_writing = self.writing_path(chunk_id).exists().await;

        let actual_offset = offset.min(file_size);
        let remaining = file_size - actual_offset;
        let to_read = max_len.min(remaining) as usize;

        let mut file = fs::File::open(&cpath).await.context("failed to open")?;
        tokio::io::AsyncSeekExt::seek(
            &mut file,
            tokio::io::SeekFrom::Start(actual_offset),
        )
        .await?;
        let mut buf = vec![0u8; to_read];
        file.read_exact(&mut buf).await?;

        let end_of_file = actual_offset + to_read as u64 >= file_size;
        let done = !is_writing && end_of_file;

        Ok((buf, file_size, done))
    }

    pub async fn delete(&self, chunk_id: &str) -> Result<()> {
        let _ = fs::remove_file(self.chunk_path(chunk_id)).await;
        let _ = fs::remove_file(self.writing_path(chunk_id)).await;
        Ok(())
    }

    /// Clean up stale .writing files on startup.
    pub async fn cleanup_stale_writes(&self) -> Result<Vec<String>> {
        let dir = self.data_dir.join("binary");
        let mut cleaned = Vec::new();
        if !dir.exists().await {
            return Ok(cleaned);
        }
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await.map_or(false, |t| t.is_dir()) {
                let mut sub = fs::read_dir(entry.path()).await?;
                while let Some(file) = sub.next_entry().await? {
                    let name = file.file_name().to_string_lossy().to_string();
                    if name.ends_with(".writing") {
                        let chunk_id = name.trim_end_matches(".writing");
                        let _ = fs::remove_file(file.path()).await;
                        let _ = fs::remove_file(
                            file.path().with_file_name(chunk_id),
                        )
                        .await;
                        cleaned.push(chunk_id.to_string());
                    }
                }
            }
        }
        Ok(cleaned)
    }
}

/// Extension trait: check if a path exists (tokio doesn't have this directly).
trait PathExists {
    async fn exists(&self) -> bool;
}

impl PathExists for PathBuf {
    async fn exists(&self) -> bool {
        fs::metadata(self).await.is_ok()
    }
}
