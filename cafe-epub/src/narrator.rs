use crate::epub::Book;
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Per-session narration state.
#[derive(Debug, Default)]
pub struct SessionState {
    /// The loaded book, if any.
    pub book: Option<Book>,
    /// Current chapter index (0-based) into `book.chapters`.
    pub current: usize,
    /// Whether a chapter has been read since the book was loaded.
    pub started: bool,
    /// ID of the session chunk announcing the book blob (for persistence).
    pub blob_chunk_id: Option<String>,
    /// Original filesystem path the book was loaded from.
    pub source_path: Option<String>,
}

impl SessionState {
    /// The chapter currently being read, if a book is loaded.
    pub fn current_chapter(&self) -> Option<&crate::epub::Chapter> {
        self.book.as_ref()?.chapters.get(self.current)
    }

    /// Move to the next chapter. Returns the index to read, or `None` if already
    /// at the last chapter. The first call reads chapter 1 (index 0).
    pub fn next(&mut self) -> Option<usize> {
        let len = self.book.as_ref()?.chapters.len();
        if !self.started {
            self.started = true;
            return Some(self.current);
        }
        if self.current + 1 < len {
            self.current += 1;
            Some(self.current)
        } else {
            None
        }
    }

    /// Move to the previous chapter. Returns the index to read, or `None` if
    /// already at the first chapter.
    pub fn prev(&mut self) -> Option<usize> {
        if !self.started || self.current == 0 {
            None
        } else {
            self.current -= 1;
            Some(self.current)
        }
    }

    /// Jump to a specific chapter (1-based, as the user types it).
    /// Returns the 0-based index, or `None` if out of range.
    pub fn goto(&mut self, chapter_one_based: usize) -> Option<usize> {
        let len = self.book.as_ref()?.chapters.len();
        if chapter_one_based >= 1 && chapter_one_based <= len {
            self.started = true;
            self.current = chapter_one_based - 1;
            Some(self.current)
        } else {
            None
        }
    }
}

/// Shared registry of per-session narration state.
#[derive(Debug, Default)]
pub struct Narrator {
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl Narrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutate the state for a session (inserting a default if absent).
    pub async fn with_session<T>(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionState) -> T,
    ) -> T {
        let mut guard = self.sessions.lock().await;
        let state = guard.entry(session_id.to_string()).or_default();
        f(state)
    }

    /// Read the state for a session without mutating it.
    pub async fn peek<T>(
        &self,
        session_id: &str,
        f: impl FnOnce(&SessionState) -> T,
    ) -> T {
        let guard = self.sessions.lock().await;
        let state = guard.get(session_id);
        match state {
            Some(state) => f(state),
            None => f(&SessionState::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::{Book, Chapter};

    fn book(n: usize) -> Book {
        Book {
            title: "Test".into(),
            creator: None,
            chapters: (0..n)
                .map(|i| Chapter {
                    title: format!("Chapter {}", i + 1),
                    text: format!("text {}", i + 1),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn next_moves_forward() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(3))).await;
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(0));
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(1));
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(2));
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, None);
    }

    #[tokio::test]
    async fn prev_clamps_at_zero() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(3))).await;
        narrator.with_session("s1", |s| s.next()).await;
        assert_eq!(narrator.with_session("s1", |s| s.prev()).await, None);
        narrator.with_session("s1", |s| {
            s.current = 2;
            s.started = true;
        })
        .await;
        assert_eq!(narrator.with_session("s1", |s| s.prev()).await, Some(1));
    }

    #[tokio::test]
    async fn first_next_reads_chapter_one() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(3))).await;
        // !next right after !load must read chapter 1, not chapter 2.
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(0));
        let title = narrator
            .peek("s1", |s| s.current_chapter().map(|c| c.title.clone()))
            .await;
        assert_eq!(title.as_deref(), Some("Chapter 1"));
    }

    #[tokio::test]
    async fn goto_is_one_based() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(5))).await;
        assert_eq!(narrator.with_session("s1", |s| s.goto(3)).await, Some(2));
        assert_eq!(narrator.with_session("s1", |s| s.goto(0)).await, None);
        assert_eq!(narrator.with_session("s1", |s| s.goto(6)).await, None);
    }

    #[tokio::test]
    async fn current_chapter_reflects_position() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(3))).await;
        narrator.with_session("s1", |s| {
            s.current = 1;
            s.started = true;
        })
        .await;
        let title = narrator
            .peek("s1", |s| s.current_chapter().map(|c| c.title.clone()))
            .await;
        assert_eq!(title.as_deref(), Some("Chapter 2"));
    }

    #[tokio::test]
    async fn sessions_are_isolated() {
        let narrator = Narrator::new();
        narrator.with_session("s1", |s| s.book = Some(book(2))).await;
        narrator.with_session("s2", |s| s.book = Some(book(9))).await;
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(0));
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, Some(1));
        assert_eq!(narrator.with_session("s1", |s| s.next()).await, None);
        assert_eq!(narrator.with_session("s2", |s| s.current).await, 0);
    }
}
