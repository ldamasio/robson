/*
Package cmd - Monitoring commands

These commands provide real-time visibility into positions, prices, and account
summary data for the production dashboard.
*/
package cmd

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/cobra"
)

const (
	defaultAPIBaseURL = "http://localhost:8000"
	green             = "\033[32m"
	red               = "\033[31m"
	reset             = "\033[0m"
)

type positionsResponse struct {
	Positions []position `json:"positions"`
}

type position struct {
	ID                      int    `json:"id"`
	OperationID             int    `json:"operation_id"`
	Symbol                  string `json:"symbol"`
	Side                    string `json:"side"`
	Quantity                string `json:"quantity"`
	EntryPrice              string `json:"entry_price"`
	CurrentPrice            string `json:"current_price"`
	UnrealizedPnL           string `json:"unrealized_pnl"`
	UnrealizedPnLPercent    string `json:"unrealized_pnl_percent"`
	StopLoss                string `json:"stop_loss"`
	TakeProfit              string `json:"take_profit"`
	DistanceToStopPercent   string `json:"distance_to_stop_percent"`
	DistanceToTargetPercent string `json:"distance_to_target_percent"`
	Status                  string `json:"status"`
}

type priceResponse struct {
	Symbol    string `json:"symbol"`
	Bid       string `json:"bid"`
	Ask       string `json:"ask"`
	Last      string `json:"last"`
	Timestamp int64  `json:"timestamp"`
	Source    string `json:"source"`
}

var positionsCmd = &cobra.Command{
	Use:   "positions",
	Short: "List active positions with P&L",
	RunE: func(cmd *cobra.Command, args []string) error {
		body, _, err := fetchAPI(cmd, "/api/portfolio/positions/")
		if err != nil {
			return err
		}

		var payload positionsResponse
		if err := decodeJSON(body, &payload); err != nil {
			return fmt.Errorf("failed to parse positions response: %w", err)
		}

		if jsonOutput {
			return outputJSON(payload)
		}

		if len(payload.Positions) == 0 {
			fmt.Println("No active positions.")
			return nil
		}

		for _, pos := range payload.Positions {
			printPosition(pos)
			fmt.Println()
		}
		return nil
	},
}

var priceCmd = &cobra.Command{
	Use:   "price <symbol>",
	Short: "Show current market price",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		symbol := normalizeSymbol(args[0])
		watch, _ := cmd.Flags().GetBool("watch")

		if watch {
			ticker := time.NewTicker(1 * time.Second)
			defer ticker.Stop()

			for {
				if !jsonOutput {
					clearScreen()
				}
				if err := printPrice(cmd, symbol); err != nil {
					return err
				}
				if jsonOutput {
					fmt.Println()
				}
				<-ticker.C
			}
		}

		return printPrice(cmd, symbol)
	},
}

var accountCmd = &cobra.Command{
	Use:   "account",
	Short: "Show account summary and exposure",
	RunE: func(cmd *cobra.Command, args []string) error {
		positionsPayload, positionsValue, err := fetchPositions(cmd)
		if err != nil {
			return err
		}

		patrimonyData, err := fetchJSONMap(cmd, "/api/portfolio/patrimony/")
		if err != nil {
			return err
		}

		balanceData, err := fetchBalance(cmd)
		if err != nil {
			return err
		}

		totalBalance := readNumber(patrimonyData["patrimony"])
		positionsValuePtr := &positionsValue
		availableBalance := deriveAvailableBalance(balanceData, totalBalance, positionsValuePtr)
		exposurePercent := computeExposurePercent(totalBalance, positionsValuePtr)

		if jsonOutput {
			return outputJSON(map[string]interface{}{
				"total_balance":     formatOptionalNumber(totalBalance),
				"available_balance": formatOptionalNumber(availableBalance),
				"positions_value":   formatOptionalNumber(positionsValuePtr),
				"exposure_percent":  formatOptionalNumber(exposurePercent),
				"num_positions":     len(positionsPayload.Positions),
				"balance_raw":       balanceData,
				"patrimony_raw":     patrimonyData,
			})
		}

		fmt.Println("╔════════════════════════════════════════════════════════════╗")
		fmt.Println("║                    ACCOUNT SUMMARY                        ║")
		fmt.Println("╚════════════════════════════════════════════════════════════╝")
		fmt.Printf("Total Balance:     %s\n", formatOptionalUSD(totalBalance))
		fmt.Printf("Positions Value:   %s\n", formatOptionalUSD(positionsValuePtr))
		fmt.Printf("Available Balance: %s\n", formatOptionalUSD(availableBalance))
		fmt.Printf("Exposure:          %s\n", formatOptionalPercent(exposurePercent))
		fmt.Printf("Active Positions:  %d\n", len(positionsPayload.Positions))
		return nil
	},
}

func init() {
	positionsCmd.Flags().String("api-base-url", "", "Base URL for the backend API (env: ROBSON_API_BASE_URL)")
	positionsCmd.Flags().String("token", "", "JWT access token (env: ROBSON_API_TOKEN)")

	priceCmd.Flags().Bool("watch", false, "Poll price every second")
	priceCmd.Flags().String("api-base-url", "", "Base URL for the backend API (env: ROBSON_API_BASE_URL)")
	priceCmd.Flags().String("token", "", "JWT access token (env: ROBSON_API_TOKEN)")

	accountCmd.Flags().String("api-base-url", "", "Base URL for the backend API (env: ROBSON_API_BASE_URL)")
	accountCmd.Flags().String("token", "", "JWT access token (env: ROBSON_API_TOKEN)")

	rootCmd.AddCommand(positionsCmd)
	rootCmd.AddCommand(priceCmd)
	rootCmd.AddCommand(accountCmd)
}

func fetchPositions(cmd *cobra.Command) (positionsResponse, float64, error) {
	body, _, err := fetchAPI(cmd, "/api/portfolio/positions/")
	if err != nil {
		return positionsResponse{}, 0, err
	}

	var payload positionsResponse
	if err := decodeJSON(body, &payload); err != nil {
		return positionsResponse{}, 0, fmt.Errorf("failed to parse positions response: %w", err)
	}

	var positionsValue float64
	for _, pos := range payload.Positions {
		price := readNumber(pos.CurrentPrice)
		qty := readNumber(pos.Quantity)
		if price != nil && qty != nil {
			positionsValue += *price * *qty
		}
	}

	return payload, positionsValue, nil
}

func fetchBalance(cmd *cobra.Command) (map[string]interface{}, error) {
	payload, status, err := fetchAPI(cmd, "/api/trade/balance/")
	if err == nil {
		return decodeJSONMap(payload)
	}
	if status == http.StatusNotFound {
		fallbackPayload, _, fallbackErr := fetchAPI(cmd, "/api/account/balance/")
		if fallbackErr != nil {
			return nil, fallbackErr
		}
		return decodeJSONMap(fallbackPayload)
	}
	return nil, err
}

func decodeJSONMap(body []byte) (map[string]interface{}, error) {
	var payload map[string]interface{}
	if err := decodeJSON(body, &payload); err != nil {
		return nil, err
	}
	return payload, nil
}

func fetchJSONMap(cmd *cobra.Command, path string) (map[string]interface{}, error) {
	body, _, err := fetchAPI(cmd, path)
	if err != nil {
		return nil, err
	}
	return decodeJSONMap(body)
}

func printPosition(pos position) {
	sideLabel := "LONG"
	if strings.ToUpper(pos.Side) == "SELL" {
		sideLabel = "SHORT"
	}

	pnlValue := readNumber(pos.UnrealizedPnL)
	pnlPercentValue := readNumber(pos.UnrealizedPnLPercent)

	pnlLine := fmt.Sprintf("%s (%s)", formatSignedUSD(pnlValue), formatSignedPercent(pnlPercentValue))
	pnlLine = colorizeNumber(pnlValue, pnlLine)

	currentLine := fmt.Sprintf("$%s (%s)", pos.CurrentPrice, formatSignedPercent(pnlPercentValue))
	currentLine = colorizeNumber(pnlPercentValue, currentLine)

	stopLine := "N/A"
	if pos.StopLoss != "" && pos.DistanceToStopPercent != "" {
		stopLine = fmt.Sprintf("$%s (%s%% away)", pos.StopLoss, pos.DistanceToStopPercent)
	}

	targetLine := "N/A"
	if pos.TakeProfit != "" && pos.DistanceToTargetPercent != "" {
		targetLine = fmt.Sprintf("$%s (%s%% to go)", pos.TakeProfit, pos.DistanceToTargetPercent)
	}

	fmt.Println("╔════════════════════════════════════════════════════════════╗")
	fmt.Println("║                    ACTIVE POSITION                        ║")
	fmt.Println("╚════════════════════════════════════════════════════════════╝")
	fmt.Printf("Symbol:   %s\n", pos.Symbol)
	fmt.Printf("Side:     %s\n", sideLabel)
	fmt.Printf("Quantity: %s\n", pos.Quantity)
	fmt.Printf("Entry:    $%s\n", pos.EntryPrice)
	fmt.Printf("Current:  %s\n", currentLine)
	fmt.Printf("P&L:      %s\n", pnlLine)
	fmt.Printf("Stop:     %s\n", stopLine)
	fmt.Printf("Target:   %s\n", targetLine)
}

func printPrice(cmd *cobra.Command, symbol string) error {
	body, _, err := fetchAPI(cmd, fmt.Sprintf("/api/market/price/%s/", symbol))
	if err != nil {
		return err
	}

	var payload priceResponse
	if err := decodeJSON(body, &payload); err != nil {
		return fmt.Errorf("failed to parse price response: %w", err)
	}

	if jsonOutput {
		return outputJSON(payload)
	}

	bidValue := readNumber(payload.Bid)
	askValue := readNumber(payload.Ask)
	spread := computeSpread(bidValue, askValue)
	fmt.Printf("%s: Bid %s | Ask %s | Spread %s\n",
		payload.Symbol,
		formatOptionalUSD(bidValue),
		formatOptionalUSD(askValue),
		formatOptionalUSD(spread),
	)
	return nil
}

func fetchAPI(cmd *cobra.Command, path string) ([]byte, int, error) {
	baseURL := resolveBaseURL(cmd)
	token := resolveToken(cmd)
	if token == "" {
		return nil, 0, errors.New("missing API token (set --token or ROBSON_API_TOKEN)")
	}

	url := strings.TrimRight(baseURL, "/") + path
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return nil, 0, err
	}
	req.Header.Set("Authorization", "Bearer "+token)

	client := &http.Client{Timeout: 10 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, resp.StatusCode, err
	}

	if resp.StatusCode >= http.StatusBadRequest {
		return nil, resp.StatusCode, fmt.Errorf("API request failed (%d): %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}

	return body, resp.StatusCode, nil
}

func decodeJSON(body []byte, output interface{}) error {
	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.UseNumber()
	return decoder.Decode(output)
}

func resolveBaseURL(cmd *cobra.Command) string {
	flagValue, _ := cmd.Flags().GetString("api-base-url")
	if flagValue != "" {
		return flagValue
	}
	if envValue := os.Getenv("ROBSON_API_BASE_URL"); envValue != "" {
		return envValue
	}
	return defaultAPIBaseURL
}

func resolveToken(cmd *cobra.Command) string {
	flagValue, _ := cmd.Flags().GetString("token")
	if flagValue != "" {
		return flagValue
	}
	if envValue := os.Getenv("ROBSON_API_TOKEN"); envValue != "" {
		return envValue
	}
	return os.Getenv("ROBSON_JWT")
}

func normalizeSymbol(symbol string) string {
	normalized := strings.ReplaceAll(symbol, "/", "")
	return strings.ToUpper(normalized)
}

func readNumber(value interface{}) *float64 {
	if value == nil {
		return nil
	}
	switch v := value.(type) {
	case string:
		if v == "" {
			return nil
		}
		num, err := strconv.ParseFloat(v, 64)
		if err != nil {
			return nil
		}
		return &num
	case json.Number:
		num, err := v.Float64()
		if err != nil {
			return nil
		}
		return &num
	case float64:
		return &v
	default:
		return nil
	}
}

func formatSignedUSD(value *float64) string {
	if value == nil {
		return "N/A"
	}
	sign := ""
	if *value > 0 {
		sign = "+"
	} else if *value < 0 {
		sign = "-"
	}
	return fmt.Sprintf("%s$%.2f", sign, math.Abs(*value))
}

func formatSignedPercent(value *float64) string {
	if value == nil {
		return "N/A"
	}
	sign := ""
	if *value > 0 {
		sign = "+"
	} else if *value < 0 {
		sign = "-"
	}
	return fmt.Sprintf("%s%.2f%%", sign, math.Abs(*value))
}

func formatOptionalUSD(value *float64) string {
	if value == nil {
		return "N/A"
	}
	return fmt.Sprintf("$%.2f", *value)
}

func formatOptionalPercent(value *float64) string {
	if value == nil {
		return "N/A"
	}
	return fmt.Sprintf("%.2f%%", *value)
}

func formatOptionalNumber(value *float64) interface{} {
	if value == nil {
		return nil
	}
	return fmt.Sprintf("%.2f", *value)
}

func computeSpread(bid, ask *float64) *float64 {
	if bid == nil || ask == nil {
		return nil
	}
	spread := *ask - *bid
	return &spread
}

func computeExposurePercent(total, positions *float64) *float64 {
	if total == nil || positions == nil || *total == 0 {
		return nil
	}
	value := (*positions / *total) * 100
	return &value
}

func deriveAvailableBalance(balance map[string]interface{}, total, positions *float64) *float64 {
	spot := readNumber(balance["spot"])
	isolated := readNumber(balance["isolated_margin"])
	if spot != nil || isolated != nil {
		value := 0.0
		if spot != nil {
			value += *spot
		}
		if isolated != nil {
			value += *isolated
		}
		return &value
	}
	if total != nil && positions != nil && *total > 0 {
		value := *total - *positions
		return &value
	}
	return nil
}

func colorizeNumber(value *float64, text string) string {
	if value == nil || os.Getenv("ROBSON_NO_COLOR") != "" {
		return text
	}
	if *value > 0 {
		return green + text + reset
	}
	if *value < 0 {
		return red + text + reset
	}
	return text
}

func clearScreen() {
	fmt.Print("\033[H\033[2J")
}
