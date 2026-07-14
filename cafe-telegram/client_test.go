package main

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// newSSEServer returns an httptest server that streams the given SSE body.
func newSSEServer(t *testing.T, body string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.Write([]byte(body))
	}))
}

// Bug A: a SSE data: line larger than bufio's default 64KB token must still be
// parsed rather than aborting the stream with ErrTooLong.
func TestStreamChatLargeLine(t *testing.T) {
	big := strings.Repeat("z", 100*1024)
	body := fmt.Sprintf(
		"data: {\"content_type\":\"text\",\"content\":%q,\"annotations\":{}}\n\n"+
			"data: {\"content_type\":\"text\",\"content\":\"\",\"annotations\":{\"chat.stream_complete\":true}}\n\n",
		big,
	)
	server := newSSEServer(t, body)
	defer server.Close()

	client := NewCafeClient(server.URL, "tok")
	out := make(chan Chunk, 8)
	if err := client.StreamChat("sess", "hi", out); err != nil {
		t.Fatalf("StreamChat returned error: %v", err)
	}
	close(out)

	for c := range out {
		if c.Content != nil && *c.Content == big {
			return
		}
	}
	t.Fatalf("expected chunk with %d-char content to be parsed", len(big))
}

// Bug E: a CreateSession response missing the "id" field must return an error
// rather than ("", nil).
func TestCreateSessionMissingID(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"session_id":"abc"}`))
	}))
	defer server.Close()

	client := NewCafeClient(server.URL, "tok")
	id, err := client.CreateSession("default")
	if err == nil {
		t.Fatalf("expected error when response is missing \"id\", got id=%q", id)
	}
	if id != "" {
		t.Fatalf("expected empty id on error, got %q", id)
	}
}
