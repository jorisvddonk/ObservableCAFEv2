package main

import (
	"os"
	"strings"
)

type Config struct {
	TelegramToken string
	CafeServerURL string
	CafeToken     string
	DBPath        string
	TrustedUsers  []string
}

func LoadConfig() Config {
	trustedRaw := os.Getenv("TELEGRAM_TRUSTED_USERS")
	var trusted []string
	for _, u := range strings.Split(trustedRaw, ",") {
		u = strings.TrimSpace(u)
		if u != "" {
			trusted = append(trusted, u)
		}
	}

	serverURL := os.Getenv("CAFE_SERVER_URL")
	if serverURL == "" {
		serverURL = "http://localhost:4000"
	}

	dbPath := os.Getenv("TELEGRAM_DB_PATH")
	if dbPath == "" {
		dbPath = "./telegram.db"
	}

	return Config{
		TelegramToken: os.Getenv("TELEGRAM_TOKEN"),
		CafeServerURL: serverURL,
		CafeToken:     os.Getenv("CAFE_TOKEN"),
		DBPath:        dbPath,
		TrustedUsers:  trusted,
	}
}
