package httpx

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net"
	"net/http"
	"time"

	clierr "github.com/gustavo/defi-cli/internal/errors"
)

type Client struct {
	httpClient *http.Client
	retries    int
	userAgent  string
}

func New(timeout time.Duration, retries int) *Client {
	if retries < 0 {
		retries = 0
	}
	return &Client{
		httpClient: &http.Client{Timeout: timeout},
		retries:    retries,
		userAgent:  "defi-cli/1.0",
	}
}

func (c *Client) DoJSON(ctx context.Context, req *http.Request, out any) (http.Header, error) {
	if req.Header.Get("Accept") == "" {
		req.Header.Set("Accept", "application/json")
	}
	if req.Header.Get("User-Agent") == "" {
		req.Header.Set("User-Agent", c.userAgent)
	}

	var lastErr error
	for attempt := 0; attempt <= c.retries; attempt++ {
		if attempt > 0 {
			select {
			case <-ctx.Done():
				return nil, clierr.Wrap(clierr.CodeUnavailable, "request cancelled", ctx.Err())
			case <-time.After(backoff(attempt)):
			}
		}

		cloneReq := req.Clone(ctx)
		if req.Body != nil && req.GetBody != nil {
			body, err := req.GetBody()
			if err != nil {
				return nil, clierr.Wrap(clierr.CodeInternal, "clone request body", err)
			}
			cloneReq.Body = body
		}

		resp, err := c.httpClient.Do(cloneReq)
		if err != nil {
			lastErr = mapNetError(err)
			if attempt < c.retries {
				continue
			}
			return nil, lastErr
		}

		buf, readErr := io.ReadAll(resp.Body)
		_ = resp.Body.Close()
		if readErr != nil {
			return resp.Header, clierr.Wrap(clierr.CodeUnavailable, "read provider response", readErr)
		}

		if resp.StatusCode == http.StatusTooManyRequests {
			lastErr = clierr.New(clierr.CodeRateLimited, "provider rate limited request")
			if attempt < c.retries {
				continue
			}
			return resp.Header, lastErr
		}

		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
			return resp.Header, clierr.New(clierr.CodeAuth, "provider authentication failed")
		}

		if resp.StatusCode >= http.StatusInternalServerError {
			lastErr = clierr.New(clierr.CodeUnavailable, fmt.Sprintf("provider unavailable (status %d)", resp.StatusCode))
			if attempt < c.retries {
				continue
			}
			return resp.Header, lastErr
		}

		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			return resp.Header, clierr.New(clierr.CodeUnsupported, fmt.Sprintf("provider returned unexpected status %d", resp.StatusCode))
		}

		if out == nil {
			return resp.Header, nil
		}
		if len(bytes.TrimSpace(buf)) == 0 {
			return resp.Header, clierr.New(clierr.CodeUnavailable, "provider returned empty response")
		}
		if err := json.Unmarshal(buf, out); err != nil {
			return resp.Header, clierr.Wrap(clierr.CodeUnavailable, "decode provider JSON", err)
		}
		return resp.Header, nil
	}

	if lastErr != nil {
		return nil, lastErr
	}
	return nil, clierr.New(clierr.CodeUnavailable, "request failed")
}

func DoBodyJSON(ctx context.Context, c *Client, method, url string, body []byte, headers map[string]string, out any) (http.Header, error) {
	var reader io.Reader
	if body != nil {
		reader = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, url, reader)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeInternal, "build request", err)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
		req.GetBody = func() (io.ReadCloser, error) {
			return io.NopCloser(bytes.NewReader(body)), nil
		}
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	return c.DoJSON(ctx, req, out)
}

func mapNetError(err error) error {
	if nerr, ok := err.(net.Error); ok {
		if nerr.Timeout() {
			return clierr.Wrap(clierr.CodeUnavailable, "provider timeout", err)
		}
	}
	return clierr.Wrap(clierr.CodeUnavailable, "provider request failed", err)
}

func backoff(attempt int) time.Duration {
	base := 120 * time.Millisecond
	d := base * time.Duration(1<<uint(attempt-1))
	if d > 2*time.Second {
		d = 2 * time.Second
	}
	jitter := time.Duration(rand.Intn(75)) * time.Millisecond
	return d + jitter
}
