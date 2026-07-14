package main

import (
	"database/sql"
	"time"

	_ "modernc.org/sqlite"
)

type DB struct {
	db *sql.DB
}

func OpenDB(path string) (*DB, error) {
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, err
	}
	d := &DB{db: db}
	return d, d.migrate()
}

func (d *DB) migrate() error {
	_, err := d.db.Exec(`
		CREATE TABLE IF NOT EXISTS user_sessions (
			telegram_user_id INTEGER NOT NULL,
			session_id       TEXT NOT NULL,
			is_active        INTEGER NOT NULL DEFAULT 0,
			PRIMARY KEY (telegram_user_id, session_id)
		);
		CREATE TABLE IF NOT EXISTS subscriptions (
			telegram_user_id INTEGER NOT NULL,
			session_id       TEXT NOT NULL,
			PRIMARY KEY (telegram_user_id, session_id)
		);
	`)
	return err
}

func (d *DB) GetActiveSession(userID int64) (string, bool) {
	var sessionID string
	err := d.db.QueryRow(
		"SELECT session_id FROM user_sessions WHERE telegram_user_id = ? AND is_active = 1",
		userID,
	).Scan(&sessionID)
	if err != nil {
		return "", false
	}
	return sessionID, true
}

func (d *DB) SetActiveSession(userID int64, sessionID string) error {
	tx, err := d.db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()

	// Clear existing active
	_, err = tx.Exec(
		"UPDATE user_sessions SET is_active = 0 WHERE telegram_user_id = ?",
		userID,
	)
	if err != nil {
		return err
	}

	// Upsert new active session
	_, err = tx.Exec(
		`INSERT INTO user_sessions (telegram_user_id, session_id, is_active)
		 VALUES (?, ?, 1)
		 ON CONFLICT(telegram_user_id, session_id) DO UPDATE SET is_active = 1`,
		userID, sessionID,
	)
	if err != nil {
		return err
	}
	return tx.Commit()
}

func (d *DB) AddSubscription(userID int64, sessionID string) error {
	_, err := d.db.Exec(
		`INSERT OR IGNORE INTO subscriptions (telegram_user_id, session_id) VALUES (?, ?)`,
		userID, sessionID,
	)
	return err
}

func (d *DB) RemoveSubscription(userID int64, sessionID string) error {
	_, err := d.db.Exec(
		"DELETE FROM subscriptions WHERE telegram_user_id = ? AND session_id = ?",
		userID, sessionID,
	)
	return err
}

func (d *DB) GetSubscriptions(userID int64) ([]string, error) {
	rows, err := d.db.Query(
		"SELECT session_id FROM subscriptions WHERE telegram_user_id = ?",
		userID,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var sessions []string
	for rows.Next() {
		var s string
		if err := rows.Scan(&s); err != nil {
			return sessions, err
		}
		sessions = append(sessions, s)
	}
	return sessions, rows.Err()
}

func nowMs() int64 {
	return time.Now().UnixMilli()
}
