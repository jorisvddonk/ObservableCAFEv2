package main

import (
	"fmt"
	"strings"
	"testing"

	tgbotapi "github.com/go-telegram-bot-api/telegram-bot-api/v5"
)

// mockBotAPI records every message it is asked to send and optionally fails
// when a message exceeds Telegram's length limit (simulating Telegram's 4096
// char cap). It implements BotAPI for tests.
type mockBotAPI struct {
	sent       []string
	failIfLong bool
}

func (m *mockBotAPI) Send(c tgbotapi.Chattable) (tgbotapi.Message, error) {
	var text string
	switch v := c.(type) {
	case tgbotapi.MessageConfig:
		text = v.Text
	case tgbotapi.EditMessageTextConfig:
		text = v.Text
	}
	m.sent = append(m.sent, text)
	if m.failIfLong && len(text) > maxTelegramMessageLen {
		return tgbotapi.Message{}, fmt.Errorf("message too long: %d chars", len(text))
	}
	return tgbotapi.Message{}, nil
}

func (m *mockBotAPI) GetUpdatesChan(config tgbotapi.UpdateConfig) tgbotapi.UpdatesChannel {
	ch := make(chan tgbotapi.Update)
	close(ch)
	return ch
}

func newTestDB(t *testing.T) *DB {
	t.Helper()
	db, err := OpenDB(t.TempDir() + "/test.db")
	if err != nil {
		t.Fatalf("OpenDB: %v", err)
	}
	return db
}

// Bug B: a message with From == nil (anonymous group post) must not panic.
func TestHandleUpdateNilFromDoesNotPanic(t *testing.T) {
	b := &Bot{
		api:    &mockBotAPI{},
		client: NewCafeClient("http://example.invalid", "tok"),
		db:     newTestDB(t),
		cfg:    Config{TrustedUsers: []string{"123"}},
	}
	update := tgbotapi.Update{
		Message: &tgbotapi.Message{
			From:      nil,
			Chat:      &tgbotapi.Chat{ID: 1},
			MessageID: 1,
		},
	}
	// A panic here would fail the test (and crash the process in production).
	b.handleUpdate(update)
}

// Bug C: a reply longer than 4096 chars must be split, not silently dropped.
func TestReplySplitsLongMessage(t *testing.T) {
	mock := &mockBotAPI{failIfLong: true}
	b := &Bot{api: mock, cfg: Config{}}
	msg := &tgbotapi.Message{
		From:      &tgbotapi.User{ID: 1},
		Chat:      &tgbotapi.Chat{ID: 1},
		MessageID: 1,
	}
	long := strings.Repeat("x", maxTelegramMessageLen+100)
	b.reply(msg, long)

	if len(mock.sent) < 2 {
		t.Fatalf("expected long reply to be split into multiple sends, got %d", len(mock.sent))
	}
	for _, s := range mock.sent {
		if len(s) > maxTelegramMessageLen {
			t.Fatalf("reply chunk exceeds Telegram limit: %d chars", len(s))
		}
	}
}

// Bug C: streamToTelegram's final edit must split an over-length reply.
func TestStreamToTelegramSplitsLongReply(t *testing.T) {
	mock := &mockBotAPI{failIfLong: true}
	big := strings.Repeat("y", maxTelegramMessageLen+200)
	body := fmt.Sprintf(
		"data: {\"content_type\":\"text\",\"content\":%q,\"annotations\":{}}\n\n"+
			"data: {\"content_type\":\"text\",\"content\":\"\",\"annotations\":{\"chat.stream_complete\":true}}\n\n",
		big,
	)
	server := newSSEServer(t, body)
	client := NewCafeClient(server.URL, "tok")
	b := &Bot{api: mock, client: client, cfg: Config{}}

	b.streamToTelegram(1, "sess", "hi")

	for _, s := range mock.sent {
		if len(s) > maxTelegramMessageLen {
			t.Fatalf("telegram chunk exceeds limit: %d chars", len(s))
		}
	}
}
