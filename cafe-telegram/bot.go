package main

import (
	"fmt"
	"log/slog"
	"strconv"
	"strings"
	"time"

	tgbotapi "github.com/go-telegram-bot-api/telegram-bot-api/v5"
)

// BotAPI is the subset of the Telegram bot API used by Bot. It is an interface
// so that Send/GetUpdatesChan can be mocked in tests. *tgbotapi.BotAPI is a
// concrete implementation.
type BotAPI interface {
	Send(c tgbotapi.Chattable) (tgbotapi.Message, error)
	GetUpdatesChan(config tgbotapi.UpdateConfig) tgbotapi.UpdatesChannel
}

// maxTelegramMessageLen is the maximum number of characters Telegram allows in
// a single message.
const maxTelegramMessageLen = 4096

type Bot struct {
	api    BotAPI
	client *CafeClient
	db     *DB
	cfg    Config
}

func NewBot(cfg Config, client *CafeClient, db *DB) (*Bot, error) {
	api, err := tgbotapi.NewBotAPI(cfg.TelegramToken)
	if err != nil {
		return nil, err
	}
	slog.Info("telegram bot authorised", "username", api.Self.UserName)
	return &Bot{api: api, client: client, db: db, cfg: cfg}, nil
}

func (b *Bot) Run() {
	u := tgbotapi.NewUpdate(0)
	u.Timeout = 60
	updates := b.api.GetUpdatesChan(u)

	for update := range updates {
		if update.Message == nil {
			continue
		}
		go b.handleUpdate(update)
	}
}

func (b *Bot) handleUpdate(update tgbotapi.Update) {
	msg := update.Message
	if !b.isTrusted(msg.From) {
		b.reply(msg, "You are not authorised to use this bot.")
		return
	}

	if msg.IsCommand() {
		b.handleCommand(msg)
	} else {
		b.handleText(msg)
	}
}

func (b *Bot) isTrusted(user *tgbotapi.User) bool {
	if user == nil {
		return false
	}
	if len(b.cfg.TrustedUsers) == 0 {
		return true // open access if no list configured
	}
	idStr := strconv.FormatInt(int64(user.ID), 10)
	for _, u := range b.cfg.TrustedUsers {
		if u == idStr || u == user.UserName {
			return true
		}
	}
	return false
}

// splitMessage splits text into chunks that fit within Telegram's per-message
// character limit, breaking on newlines where possible.
func splitMessage(text string) []string {
	if len(text) <= maxTelegramMessageLen {
		return []string{text}
	}
	var parts []string
	runes := []rune(text)
	for len(runes) > 0 {
		n := maxTelegramMessageLen
		if len(runes) < n {
			n = len(runes)
		}
		// Prefer to break at a newline within the chunk.
		if idx := strings.LastIndex(string(runes[:n]), "\n"); idx >= 0 {
			n = idx + 1
		}
		parts = append(parts, string(runes[:n]))
		runes = runes[n:]
	}
	return parts
}

// sendMessage sends text to a chat, splitting it into chunks no larger than
// Telegram's limit and checking each Send result.
func (b *Bot) sendMessage(chatID int64, text string, opts ...func(*tgbotapi.MessageConfig)) {
	for _, part := range splitMessage(text) {
		m := tgbotapi.NewMessage(chatID, part)
		for _, opt := range opts {
			opt(&m)
		}
		if _, err := b.api.Send(m); err != nil {
			slog.Error("failed to send message", "err", err)
		}
	}
}

func (b *Bot) reply(msg *tgbotapi.Message, text string) {
	b.sendMessage(msg.Chat.ID, text, func(m *tgbotapi.MessageConfig) {
		m.ReplyToMessageID = msg.MessageID
	})
}

func (b *Bot) handleCommand(msg *tgbotapi.Message) {
	switch msg.Command() {
	case "start":
		b.cmdStart(msg)
	case "sessions":
		b.cmdSessions(msg)
	case "new":
		b.cmdNew(msg)
	case "join":
		b.cmdJoin(msg)
	case "id":
		b.cmdID(msg)
	case "subscribe":
		b.cmdSubscribe(msg)
	case "unsubscribe":
		b.cmdUnsubscribe(msg)
	case "subscriptions":
		b.cmdSubscriptions(msg)
	case "tag":
		b.cmdTag(msg)
	default:
		b.reply(msg, "Unknown command. Try /sessions, /new, /join <id>, /id, /tag")
	}
}

func (b *Bot) cmdStart(msg *tgbotapi.Message) {
	userID := int64(msg.From.ID)
	if _, ok := b.db.GetActiveSession(userID); !ok {
		// Create a default session
		id, err := b.client.CreateSession("default")
		if err != nil {
			b.reply(msg, fmt.Sprintf("Failed to create session: %v", err))
			return
		}
		b.db.SetActiveSession(userID, id)
		b.reply(msg, fmt.Sprintf("Welcome! Created session `%s`. Send me a message to chat.", id))
	} else {
		b.reply(msg, "Welcome back! Send me a message to chat.")
	}
}

func (b *Bot) cmdSessions(msg *tgbotapi.Message) {
	sessions, err := b.client.ListSessions()
	if err != nil {
		b.reply(msg, fmt.Sprintf("Error: %v", err))
		return
	}
	if len(sessions) == 0 {
		b.reply(msg, "No sessions found. Use /new to create one.")
		return
	}
	var sb strings.Builder
	sb.WriteString("Sessions:\n")
	for _, s := range sessions {
		name := s.SessionID
		if s.DisplayName != nil {
			name = *s.DisplayName
		}
		tags := ""
		if len(s.Tags) > 0 {
			tags = fmt.Sprintf(" [%s]", strings.Join(s.Tags, ", "))
		}
		sb.WriteString(fmt.Sprintf("• `%s` — %s%s (%d msgs)\n", s.SessionID, name, tags, s.MessageCount))
	}
	sb.WriteString("\nUse /join <id> to switch.")
	b.reply(msg, sb.String())
}

func (b *Bot) cmdNew(msg *tgbotapi.Message) {
	id, err := b.client.CreateSession("default")
	if err != nil {
		b.reply(msg, fmt.Sprintf("Failed to create session: %v", err))
		return
	}
	b.db.SetActiveSession(int64(msg.From.ID), id)
	b.reply(msg, fmt.Sprintf("Created and switched to session `%s`.", id))
}

func (b *Bot) cmdJoin(msg *tgbotapi.Message) {
	args := msg.CommandArguments()
	if args == "" {
		b.reply(msg, "Usage: /join <session-id>")
		return
	}
	b.db.SetActiveSession(int64(msg.From.ID), args)
	b.reply(msg, fmt.Sprintf("Switched to session `%s`.", args))
}

func (b *Bot) cmdID(msg *tgbotapi.Message) {
	if id, ok := b.db.GetActiveSession(int64(msg.From.ID)); ok {
		b.reply(msg, fmt.Sprintf("Current session: `%s`", id))
	} else {
		b.reply(msg, "No active session. Use /new to create one.")
	}
}

func (b *Bot) cmdSubscribe(msg *tgbotapi.Message) {
	args := msg.CommandArguments()
	if args == "" {
		b.reply(msg, "Usage: /subscribe <session-id>")
		return
	}
	b.db.AddSubscription(int64(msg.From.ID), args)
	b.reply(msg, fmt.Sprintf("Subscribed to session `%s`.", args))
}

func (b *Bot) cmdUnsubscribe(msg *tgbotapi.Message) {
	args := msg.CommandArguments()
	if args == "" {
		b.reply(msg, "Usage: /unsubscribe <session-id>")
		return
	}
	b.db.RemoveSubscription(int64(msg.From.ID), args)
	b.reply(msg, fmt.Sprintf("Unsubscribed from session `%s`.", args))
}

func (b *Bot) cmdSubscriptions(msg *tgbotapi.Message) {
	subs, err := b.db.GetSubscriptions(int64(msg.From.ID))
	if err != nil || len(subs) == 0 {
		b.reply(msg, "No active subscriptions.")
		return
	}
	b.reply(msg, "Subscriptions:\n"+strings.Join(subs, "\n"))
}

func (b *Bot) handleText(msg *tgbotapi.Message) {
	userID := int64(msg.From.ID)
	sessionID, ok := b.db.GetActiveSession(userID)
	if !ok {
		// Auto-create a session
		id, err := b.client.CreateSession("default")
		if err != nil {
			b.reply(msg, fmt.Sprintf("Failed to create session: %v", err))
			return
		}
		b.db.SetActiveSession(userID, id)
		sessionID = id
	}

	b.streamToTelegram(msg.Chat.ID, sessionID, msg.Text)
}

func (b *Bot) cmdTag(msg *tgbotapi.Message) {
	args := strings.Fields(msg.CommandArguments())
	if len(args) < 2 {
		b.reply(msg, "Usage: /tag <session-id> <tag1> <tag2> ...")
		return
	}
	sessionID := args[0]
	tags := args[1:]
	if err := b.client.SetTags(sessionID, tags); err != nil {
		b.reply(msg, fmt.Sprintf("Failed to set tags: %v", err))
		return
	}
	b.reply(msg, fmt.Sprintf("Tags set for `%s`: %s", sessionID, strings.Join(tags, ", ")))
}

func (b *Bot) streamToTelegram(chatID int64, sessionID, userMessage string) {
	// Send placeholder
	placeholder, err := b.api.Send(tgbotapi.NewMessage(chatID, "…"))
	if err != nil {
		slog.Error("failed to send placeholder", "err", err)
		return
	}

	chunks := make(chan Chunk, 64)
	go func() {
		if err := b.client.StreamChat(sessionID, userMessage, chunks); err != nil {
			slog.Error("stream error", "err", err)
		}
		close(chunks)
	}()

	var accumulated strings.Builder
	lastEdit := time.Now()

	for chunk := range chunks {
		if chunk.ContentType == "binary" {
			// Send as photo/audio if possible
			if chunk.MimeType != nil && strings.HasPrefix(*chunk.MimeType, "image/") {
				slog.Info("binary image chunk received — skipping Telegram media send (not implemented)")
			}
			continue
		}
		if chunk.IsStreamComplete() {
			break
		}
		if chunk.Content != nil {
			accumulated.WriteString(*chunk.Content)
		}
		if time.Since(lastEdit) > 800*time.Millisecond && accumulated.Len() > 0 {
			edit := tgbotapi.NewEditMessageText(chatID, placeholder.MessageID, accumulated.String())
			if _, err := b.api.Send(edit); err != nil {
				slog.Error("failed to edit message", "err", err)
			}
			lastEdit = time.Now()
		}
	}

	// Final edit
	text := accumulated.String()
	if text == "" {
		text = "(no response)"
	}
	parts := splitMessage(text)
	if len(parts) == 1 {
		edit := tgbotapi.NewEditMessageText(chatID, placeholder.MessageID, parts[0])
		if _, err := b.api.Send(edit); err != nil {
			slog.Error("failed to edit message", "err", err)
		}
		return
	}
	// The reply is too long for a single message: edit the placeholder with the
	// first part and send the rest as new messages.
	edit := tgbotapi.NewEditMessageText(chatID, placeholder.MessageID, parts[0])
	if _, err := b.api.Send(edit); err != nil {
		slog.Error("failed to edit message", "err", err)
	}
	b.sendMessage(chatID, strings.Join(parts[1:], ""))
}
