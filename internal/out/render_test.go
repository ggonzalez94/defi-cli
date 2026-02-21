package out

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/gustavo/defi-cli/internal/config"
	"github.com/gustavo/defi-cli/internal/model"
)

func TestRenderJSONSelectResultsOnly(t *testing.T) {
	env := model.Envelope{
		Version: "v1",
		Success: true,
		Data:    []map[string]any{{"a": 1, "b": 2}},
		Meta:    model.EnvelopeMeta{Timestamp: time.Now()},
	}
	settings := config.Settings{OutputMode: "json", SelectFields: []string{"a"}, ResultsOnly: true}
	var buf bytes.Buffer
	if err := Render(&buf, env, settings); err != nil {
		t.Fatalf("Render failed: %v", err)
	}
	var out []map[string]any
	if err := json.Unmarshal(buf.Bytes(), &out); err != nil {
		t.Fatalf("json decode failed: %v", err)
	}
	if len(out) != 1 || out[0]["a"].(float64) != 1 {
		t.Fatalf("unexpected output: %s", buf.String())
	}
	if _, ok := out[0]["b"]; ok {
		t.Fatalf("field projection failed: %s", buf.String())
	}
}

func TestRenderPlain(t *testing.T) {
	env := model.Envelope{
		Version: "v1",
		Success: true,
		Data:    []map[string]any{{"name": "x", "score": 42}},
		Meta:    model.EnvelopeMeta{Timestamp: time.Now()},
	}
	settings := config.Settings{OutputMode: "plain", ResultsOnly: true}
	var buf bytes.Buffer
	if err := Render(&buf, env, settings); err != nil {
		t.Fatalf("Render failed: %v", err)
	}
	if !strings.Contains(buf.String(), "name=x") {
		t.Fatalf("unexpected plain output: %s", buf.String())
	}
}
