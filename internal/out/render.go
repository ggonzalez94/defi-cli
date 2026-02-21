package out

import (
	"encoding/json"
	"fmt"
	"io"
	"reflect"
	"sort"
	"strings"

	"github.com/gustavo/defi-cli/internal/config"
	"github.com/gustavo/defi-cli/internal/model"
)

func Render(w io.Writer, env model.Envelope, settings config.Settings) error {
	data := env.Data
	if len(settings.SelectFields) > 0 {
		data = project(data, settings.SelectFields)
	}

	if settings.ResultsOnly {
		if settings.OutputMode == "json" {
			enc := json.NewEncoder(w)
			enc.SetIndent("", "  ")
			return enc.Encode(data)
		}
		return renderPlain(w, data)
	}

	if settings.OutputMode == "json" {
		env.Data = data
		enc := json.NewEncoder(w)
		enc.SetIndent("", "  ")
		return enc.Encode(env)
	}

	plain := map[string]any{
		"success":  env.Success,
		"data":     data,
		"warnings": env.Warnings,
		"meta":     env.Meta,
	}
	if env.Error != nil {
		plain["error"] = env.Error
	}
	return renderPlain(w, plain)
}

func renderPlain(w io.Writer, data any) error {
	v := reflect.ValueOf(data)
	if !v.IsValid() {
		_, err := fmt.Fprintln(w, "null")
		return err
	}

	switch v.Kind() {
	case reflect.Slice, reflect.Array:
		for i := 0; i < v.Len(); i++ {
			item := normalizeValue(v.Index(i).Interface())
			line, err := toLine(item)
			if err != nil {
				return err
			}
			if _, err := fmt.Fprintln(w, line); err != nil {
				return err
			}
		}
		if v.Len() == 0 {
			_, err := fmt.Fprintln(w, "[]")
			return err
		}
		return nil
	default:
		line, err := toLine(normalizeValue(data))
		if err != nil {
			return err
		}
		_, err = fmt.Fprintln(w, line)
		return err
	}
}

func project(data any, fields []string) any {
	n := normalizeValue(data)
	switch t := n.(type) {
	case []any:
		out := make([]map[string]any, 0, len(t))
		for _, item := range t {
			m, ok := item.(map[string]any)
			if !ok {
				continue
			}
			out = append(out, projectMap(m, fields))
		}
		return out
	case map[string]any:
		return projectMap(t, fields)
	default:
		return n
	}
}

func projectMap(m map[string]any, fields []string) map[string]any {
	out := make(map[string]any, len(fields))
	for _, f := range fields {
		if v, ok := m[f]; ok {
			out[f] = v
		}
	}
	return out
}

func normalizeValue(v any) any {
	buf, err := json.Marshal(v)
	if err != nil {
		return v
	}
	var out any
	if err := json.Unmarshal(buf, &out); err != nil {
		return v
	}
	return out
}

func toLine(v any) (string, error) {
	switch t := v.(type) {
	case map[string]any:
		keys := make([]string, 0, len(t))
		for k := range t {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		parts := make([]string, 0, len(keys))
		for _, k := range keys {
			parts = append(parts, fmt.Sprintf("%s=%v", k, t[k]))
		}
		return strings.Join(parts, " "), nil
	default:
		buf, err := json.Marshal(v)
		if err != nil {
			return "", err
		}
		return string(buf), nil
	}
}
