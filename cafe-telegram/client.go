package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Chunk mirrors the cafe-types Chunk struct.
type Chunk struct {
	ID          string                 `json:"id"`
	ContentType string                 `json:"content_type"`
	Content     *string                `json:"content"`
	Data        *string                `json:"data"`
	MimeType    *string                `json:"mime_type"`
	Producer    string                 `json:"producer"`
	Annotations map[string]interface{} `json:"annotations"`
	Timestamp   int64                  `json:"timestamp"`
}

func (c *Chunk) Role() string {
	if r, ok := c.Annotations["chat.role"].(string); ok {
		return r
	}
	return ""
}

func (c *Chunk) IsStreamComplete() bool {
	if v, ok := c.Annotations["chat.stream_complete"].(bool); ok {
		return v
	}
	return false
}

// SessionInfo mirrors cafe-types SessionInfo.
type SessionInfo struct {
	SessionID    string   `json:"session_id"`
	AgentID      string   `json:"agent_id"`
	DisplayName  *string  `json:"display_name"`
	Tags         []string `json:"tags"`
	MessageCount int      `json:"message_count"`
}

type CafeClient struct {
	baseURL string
	token   string
	http    *http.Client
}

func NewCafeClient(baseURL, token string) *CafeClient {
	return &CafeClient{
		baseURL: baseURL,
		token:   token,
		http:    &http.Client{Timeout: 30 * time.Second},
	}
}

func (c *CafeClient) authHeader(req *http.Request) {
	req.Header.Set("Authorization", "Bearer "+c.token)
	req.Header.Set("Content-Type", "application/json")
}

func (c *CafeClient) ListSessions() ([]SessionInfo, error) {
	req, _ := http.NewRequest("GET", c.baseURL+"/api/sessions", nil)
	c.authHeader(req)
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var sessions []SessionInfo
	return sessions, json.NewDecoder(resp.Body).Decode(&sessions)
}

func (c *CafeClient) CreateSession(agentID string) (string, error) {
	body, _ := json.Marshal(map[string]string{"agent_id": agentID})
	req, _ := http.NewRequest("POST", c.baseURL+"/api/sessions", bytes.NewReader(body))
	c.authHeader(req)
	resp, err := c.http.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	var result map[string]interface{}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return "", err
	}
	id, _ := result["id"].(string)
	return id, nil
}

// StreamChat sends a message and streams chunks via a channel.
func (c *CafeClient) StreamChat(sessionID, message string, out chan<- Chunk) error {
	body, _ := json.Marshal(map[string]string{"content": message})
	req, _ := http.NewRequest("POST",
		fmt.Sprintf("%s/api/sessions/%s/chat", c.baseURL, sessionID),
		bytes.NewReader(body))
	c.authHeader(req)

	// Use a client without timeout for streaming
	streamClient := &http.Client{}
	resp, err := streamClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	scanner := bufio.NewScanner(resp.Body)
	for scanner.Scan() {
		line := scanner.Text()
		if !strings.HasPrefix(line, "data: ") {
			continue
		}
		data := strings.TrimPrefix(line, "data: ")
		var chunk Chunk
		if err := json.Unmarshal([]byte(data), &chunk); err != nil {
			continue
		}
		out <- chunk
		if chunk.IsStreamComplete() {
			break
		}
	}
	return scanner.Err()
}

func (c *CafeClient) SetTags(sessionID string, tags []string) error {
	body, _ := json.Marshal(map[string]interface{}{"tags": tags})
	req, _ := http.NewRequest("PATCH",
		fmt.Sprintf("%s/api/sessions/%s/tags", c.baseURL, sessionID),
		bytes.NewReader(body))
	c.authHeader(req)
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("set tags failed: %s", resp.Status)
	}
	return nil
}

func (c *CafeClient) SendChunk(sessionID string, chunk map[string]interface{}) error {
	body, _ := json.Marshal(chunk)
	req, _ := http.NewRequest("POST",
		fmt.Sprintf("%s/api/sessions/%s/chunks", c.baseURL, sessionID),
		bytes.NewReader(body))
	c.authHeader(req)
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body)
	return nil
}
