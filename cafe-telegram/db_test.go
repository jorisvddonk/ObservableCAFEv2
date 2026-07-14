package main

import (
	"testing"
)

// Bug D: a row that fails Scan must surface an error instead of being silently
// dropped.
func TestGetSubscriptionsScanError(t *testing.T) {
	db := newTestDB(t)
	userID := int64(42)

	// Recreate the subscriptions table without the NOT NULL constraint so we
	// can insert a row whose session_id is NULL. Scanning NULL into a string
	// errors, which is what triggers the dropped-row bug.
	if _, err := db.db.Exec("DROP TABLE subscriptions"); err != nil {
		t.Fatalf("drop: %v", err)
	}
	if _, err := db.db.Exec(
		"CREATE TABLE subscriptions (telegram_user_id INTEGER NOT NULL, session_id TEXT, PRIMARY KEY (telegram_user_id, session_id))",
	); err != nil {
		t.Fatalf("create: %v", err)
	}
	if _, err := db.db.Exec(
		"INSERT INTO subscriptions (telegram_user_id, session_id) VALUES (?, NULL)",
		userID,
	); err != nil {
		t.Fatalf("setup insert: %v", err)
	}

	if _, err := db.GetSubscriptions(userID); err == nil {
		t.Fatal("expected error from GetSubscriptions when a row fails to scan, got nil")
	}
}
