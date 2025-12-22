package cmd

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
)

func TestPositionsCommandJSON(t *testing.T) {
	server := newMockAPIServer()
	defer server.Close()

	output, err := runRootCommand([]string{"positions", "--json"}, server.URL)
	if err != nil {
		t.Fatalf("positions command failed: %v", err)
	}

	var payload map[string]interface{}
	if err := json.Unmarshal([]byte(output), &payload); err != nil {
		t.Fatalf("failed to parse JSON output: %v", err)
	}

	positions, ok := payload["positions"].([]interface{})
	if !ok || len(positions) != 1 {
		t.Fatalf("expected one position, got %v", payload["positions"])
	}
}

func TestPriceCommandOutput(t *testing.T) {
	server := newMockAPIServer()
	defer server.Close()

	output, err := runRootCommand([]string{"price", "BTCUSDC"}, server.URL)
	if err != nil {
		t.Fatalf("price command failed: %v", err)
	}

	if !strings.Contains(output, "BTCUSDC: Bid") {
		t.Fatalf("unexpected output: %s", output)
	}
}

func TestAccountCommandOutput(t *testing.T) {
	server := newMockAPIServer()
	defer server.Close()

	output, err := runRootCommand([]string{"account"}, server.URL)
	if err != nil {
		t.Fatalf("account command failed: %v", err)
	}

	if !strings.Contains(output, "ACCOUNT SUMMARY") {
		t.Fatalf("unexpected output: %s", output)
	}
}

func runRootCommand(args []string, baseURL string) (string, error) {
	tokens := map[string]string{
		"ROBSON_API_BASE_URL": baseURL,
		"ROBSON_API_TOKEN":    "testtoken",
		"ROBSON_NO_COLOR":     "1",
	}
	restoreEnv := setEnv(tokens)
	defer restoreEnv()

	return captureStdout(func() error {
		jsonOutput = false
		rootCmd.SetArgs(args)
		return rootCmd.Execute()
	})
}

func captureStdout(fn func() error) (string, error) {
	originalStdout := os.Stdout
	reader, writer, _ := os.Pipe()
	os.Stdout = writer

	err := fn()

	writer.Close()
	os.Stdout = originalStdout

	output, _ := io.ReadAll(reader)
	return string(output), err
}

func setEnv(values map[string]string) func() {
	previous := make(map[string]string)
	for key, value := range values {
		previous[key] = os.Getenv(key)
		_ = os.Setenv(key, value)
	}

	return func() {
		for key, value := range previous {
			if value == "" {
				_ = os.Unsetenv(key)
			} else {
				_ = os.Setenv(key, value)
			}
		}
	}
}

func newMockAPIServer() *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer testtoken" {
			w.WriteHeader(http.StatusUnauthorized)
			_, _ = w.Write([]byte(`{"error":"unauthorized"}`))
			return
		}

		switch r.URL.Path {
		case "/api/portfolio/positions/":
			_, _ = w.Write([]byte(`{
				"positions": [{
					"id": 1,
					"operation_id": 1,
					"symbol": "BTCUSDC",
					"side": "BUY",
					"quantity": "1.00000000",
					"entry_price": "100.00",
					"current_price": "110.00",
					"unrealized_pnl": "10.00",
					"unrealized_pnl_percent": "10.00",
					"stop_loss": "98.00",
					"take_profit": "104.00",
					"distance_to_stop_percent": "-10.91",
					"distance_to_target_percent": "-5.45",
					"status": "OPEN"
				}]
			}`))
	case "/api/market/price/BTCUSDC/":
			_, _ = w.Write([]byte(`{
				"symbol": "BTCUSDC",
				"bid": "100.00",
				"ask": "101.00",
				"last": "100.50",
				"timestamp": 1700000000,
				"source": "binance"
			}`))
	case "/api/trade/balance/":
		_, _ = w.Write([]byte(`{"spot": "1000.00", "isolated_margin": "0.00"}`))
	case "/api/account/balance/":
		_, _ = w.Write([]byte(`{"spot": "1000.00", "isolated_margin": "0.00"}`))
		case "/api/portfolio/patrimony/":
			_, _ = w.Write([]byte(`{"patrimony": "1000.00"}`))
		default:
			w.WriteHeader(http.StatusNotFound)
			_, _ = w.Write([]byte(`{"error":"not found"}`))
		}
	}))
}
