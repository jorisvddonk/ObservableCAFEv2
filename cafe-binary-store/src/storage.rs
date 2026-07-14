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

    pub(crate) fn writing_path(&self, chunk_id: &str) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn new_storage() -> (Storage, TempDir) {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().to_path_buf());
        (storage, dir)
    }

    async fn write_all(storage: &Storage, chunk_id: &str, data: &[u8]) {
        storage.start_write(chunk_id).await.unwrap();
        storage.append(chunk_id, 0, data, 1024 * 1024).await.unwrap();
        storage.finalize(chunk_id).await.unwrap();
    }

    #[tokio::test]
    async fn start_write_creates_file_and_writing_marker() {
        let (storage, _dir) = new_storage();
        storage.start_write("test-1").await.unwrap();

        let cpath = storage.chunk_path("test-1");
        let wpath = storage.writing_path("test-1");

        assert!(cpath.exists().await);
        assert!(wpath.exists().await);
    }

    #[tokio::test]
    async fn concurrent_write_rejected() {
        let (storage, _dir) = new_storage();
        storage.start_write("test-2").await.unwrap();
        let result = storage.start_write("test-2").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("CONFLICT"));
    }

    #[tokio::test]
    async fn write_and_finalize_removes_marker() {
        let (storage, _dir) = new_storage();
        storage.start_write("test-3").await.unwrap();
        storage.finalize("test-3").await.unwrap();

        let wpath = storage.writing_path("test-3");
        assert!(!wpath.exists().await);
    }

    #[tokio::test]
    async fn append_writes_data() {
        let (storage, _dir) = new_storage();
        let data = b"hello binary store";
        write_all(&storage, "test-4", data).await;

        let (read_back, _size, done) = storage.read("test-4", 0, 1024).await.unwrap();
        assert_eq!(read_back, data);
        assert!(done);
    }

    #[tokio::test]
    async fn resume_at_offset() {
        let (storage, _dir) = new_storage();
        storage.start_write("test-5").await.unwrap();
        storage.append("test-5", 0, b"hello ", 1024).await.unwrap();
        storage.append("test-5", 6, b"world", 1024).await.unwrap();
        storage.finalize("test-5").await.unwrap();

        let (data, _size, _done) = storage.read("test-5", 0, 1024).await.unwrap();
        assert_eq!(data, b"hello world");
    }

    #[tokio::test]
    async fn resume_without_writing_marker_fails() {
        let (storage, _dir) = new_storage();
        let result = storage.resume_write("nonexistent", 0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("NOT_FOUND"));
    }

    #[tokio::test]
    async fn read_partial_content() {
        let (storage, _dir) = new_storage();
        write_all(&storage, "test-6", b"hello world").await;

        let (data, size, done) = storage.read("test-6", 6, 5).await.unwrap();
        assert_eq!(data, b"world");
        assert_eq!(size, 11);
        assert!(done);
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let (storage, _dir) = new_storage();
        write_all(&storage, "test-7", b"data").await;
        storage.delete("test-7").await.unwrap();

        let cpath = storage.chunk_path("test-7");
        assert!(!cpath.exists().await);
    }

    #[tokio::test]
    async fn max_bytes_enforced() {
        let (storage, _dir) = new_storage();
        storage.start_write("test-8").await.unwrap();
        let result = storage.append("test-8", 0, b"x", 0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PAYLOAD_TOO_LARGE"));
    }

    #[tokio::test]
    async fn cleanup_stale_writes() {
        let (storage, _dir) = new_storage();
        storage.start_write("stale-1").await.unwrap();
        storage.start_write("stale-2").await.unwrap();

        let cleaned = storage.cleanup_stale_writes().await.unwrap();
        assert_eq!(cleaned.len(), 2);
        assert!(cleaned.contains(&"stale-1".to_string()));
        assert!(cleaned.contains(&"stale-2".to_string()));

        assert!(!storage.chunk_path("stale-1").exists().await);
        assert!(!storage.chunk_path("stale-2").exists().await);
    }

    #[test]
    fn chunk_path_includes_shard() {
        let storage = Storage::new(PathBuf::from("/data"));
        let p = storage.chunk_path("abcdef12-3456-7890-abcd-ef1234567890");
        let s = p.to_string_lossy();
        assert!(s.contains("/data/binary/ab/"));
        assert!(s.contains("abcdef12-3456-7890-abcd-ef1234567890"));
    }
}
