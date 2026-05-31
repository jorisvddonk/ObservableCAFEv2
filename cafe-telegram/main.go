package main

import (
	"log/slog"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	cfg := LoadConfig()

	if cfg.TelegramToken == "" {
		slog.Error("TELEGRAM_TOKEN is required")
		os.Exit(1)
	}
	if cfg.CafeToken == "" {
		slog.Warn("CAFE_TOKEN is empty — API calls will likely fail")
	}

	db, err := OpenDB(cfg.DBPath)
	if err != nil {
		slog.Error("failed to open database", "err", err)
		os.Exit(1)
	}

	client := NewCafeClient(cfg.CafeServerURL, cfg.CafeToken)

	bot, err := NewBot(cfg, client, db)
	if err != nil {
		slog.Error("failed to create bot", "err", err)
		os.Exit(1)
	}

	slog.Info("cafe-telegram starting",
		"server", cfg.CafeServerURL,
		"db", cfg.DBPath,
	)

	// Run bot in background
	go bot.Run()

	// Wait for shutdown signal
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit

	slog.Info("cafe-telegram shutting down")
}
