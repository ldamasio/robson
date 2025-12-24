/*
Package cmd - Margin trading commands

These commands provide real-time visibility into:
  - Account status (balances, equity)
  - Open positions with P&L
  - Margin levels and health

They delegate to Django management commands for the actual data fetching.
*/
package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"strconv"

	"github.com/spf13/cobra"
)

// marginStatusCmd shows account status via Django
var marginStatusCmd = &cobra.Command{
	Use:   "margin-status",
	Short: "Show margin account status and positions overview",
	Long: `Display a comprehensive overview of your Robson trading account.

Shows:
  - Spot balances (USDC, BTC)
  - Isolated margin balances
  - Open positions with P&L
  - Total equity

This is a READ-ONLY command that fetches live data from Binance.

Examples:
  robson status                    # Quick overview
  robson status --detailed         # With position details
  robson status --client-id 2      # For specific client`,
	RunE: func(cmd *cobra.Command, args []string) error {
		clientID, _ := cmd.Flags().GetInt("client-id")
		detailed, _ := cmd.Flags().GetBool("detailed")

		return invokeDjangoStatus(clientID, detailed, jsonOutput)
	},
}

// marginPositionsCmd shows detailed margin positions via Django
var marginPositionsCmd = &cobra.Command{
	Use:   "margin-positions",
	Short: "Show detailed isolated margin positions",
	Long: `Display detailed information about your margin positions.

Shows for each position:
  - Entry price, current price, stop-loss
  - Quantity and leverage
  - Risk amount and percentage
  - Margin level and health status
  - Unrealized P&L
  - Binance order references

Options:
  --live    Fetch real-time prices from Binance
  --all     Include closed positions
  --json    Output as JSON for scripts

Examples:
  robson positions                 # Open positions
  robson positions --live          # With real-time prices
  robson positions --all           # Include closed
  robson positions --json          # JSON for automation
  robson positions --symbol BTCUSDC`,
	RunE: func(cmd *cobra.Command, args []string) error {
		clientID, _ := cmd.Flags().GetInt("client-id")
		live, _ := cmd.Flags().GetBool("live")
		all, _ := cmd.Flags().GetBool("all")
		symbol, _ := cmd.Flags().GetString("symbol")

		return invokeDjangoPositions(clientID, live, all, symbol, jsonOutput)
	},
}

// operationsCmd shows operations with their movements
var operationsCmd = &cobra.Command{
	Use:   "operations",
	Short: "Show operations with all movements (audit trail)",
	Long: `Display trading operations with their complete movement history.

Shows for each operation:
  - Entry and exit trades
  - Transfers (Spot <-> Isolated Margin)
  - Borrows and repayments
  - Stop-loss orders
  - Fees charged

This provides complete transparency - you can see exactly what happened.

Examples:
  robson operations                 # All recent operations
  robson operations --open          # Only open operations
  robson operations --id OP-2024    # Specific operation
  robson operations --json          # JSON for automation`,
	RunE: func(cmd *cobra.Command, args []string) error {
		clientID, _ := cmd.Flags().GetInt("client-id")
		showOpen, _ := cmd.Flags().GetBool("open")
		showClosed, _ := cmd.Flags().GetBool("closed")
		operationID, _ := cmd.Flags().GetString("id")
		limit, _ := cmd.Flags().GetInt("limit")

		return invokeDjangoOperations(clientID, showOpen, showClosed, operationID, limit, jsonOutput)
	},
}

// marginBuyCmd opens a leveraged long position
var marginBuyCmd = &cobra.Command{
	Use:   "margin-buy",
	Short: "Open a leveraged LONG position with risk management",
	Long: `Open a leveraged LONG position on Binance Isolated Margin.

This command enforces the GOLDEN RULE:
  Position size = (1% of capital) / Stop distance

This ensures that if your stop-loss is hit, you lose at most 1% of your capital.

SAFE BY DEFAULT:
  - DRY-RUN is the default (simulation)
  - LIVE requires --live AND --confirm flags

Examples:
  # DRY-RUN (preview only)
  robson margin-buy --capital 100 --stop-percent 2 --leverage 3

  # LIVE execution
  robson margin-buy --capital 100 --stop-percent 2 --leverage 3 --live --confirm

  # With specific stop price
  robson margin-buy --capital 100 --stop-price 85000 --leverage 5 --live --confirm`,
	RunE: func(cmd *cobra.Command, args []string) error {
		capital, _ := cmd.Flags().GetString("capital")
		stopPercent, _ := cmd.Flags().GetString("stop-percent")
		stopPrice, _ := cmd.Flags().GetString("stop-price")
		leverage, _ := cmd.Flags().GetInt("leverage")
		symbol, _ := cmd.Flags().GetString("symbol")
		clientID, _ := cmd.Flags().GetInt("client-id")
		live, _ := cmd.Flags().GetBool("live")
		confirm, _ := cmd.Flags().GetBool("confirm")

		return invokeDjangoMarginBuy(capital, stopPercent, stopPrice, leverage, symbol, clientID, live, confirm)
	},
}

func init() {
	// Margin-status command flags
	marginStatusCmd.Flags().Int("client-id", 1, "Client ID (tenant)")
	marginStatusCmd.Flags().Bool("detailed", false, "Show detailed position information")

	// Margin-positions command flags
	marginPositionsCmd.Flags().Int("client-id", 1, "Client ID (tenant)")
	marginPositionsCmd.Flags().Bool("live", false, "Fetch real-time prices from Binance")
	marginPositionsCmd.Flags().Bool("all", false, "Include closed positions")
	marginPositionsCmd.Flags().String("symbol", "", "Filter by symbol (e.g., BTCUSDC)")

	// Operations command flags
	operationsCmd.Flags().Int("client-id", 1, "Client ID (tenant)")
	operationsCmd.Flags().Bool("open", false, "Show only open operations")
	operationsCmd.Flags().Bool("closed", false, "Show only closed operations")
	operationsCmd.Flags().String("id", "", "Show specific operation by ID")
	operationsCmd.Flags().Int("limit", 10, "Maximum number of operations to show")

	// Margin-buy command flags
	marginBuyCmd.Flags().String("capital", "", "Capital to use for position (REQUIRED)")
	marginBuyCmd.Flags().String("stop-percent", "2", "Stop-loss as percentage below entry")
	marginBuyCmd.Flags().String("stop-price", "", "Exact stop-loss price (overrides stop-percent)")
	marginBuyCmd.Flags().Int("leverage", 3, "Leverage multiplier (2, 3, 5, or 10)")
	marginBuyCmd.Flags().String("symbol", "BTCUSDC", "Trading pair")
	marginBuyCmd.Flags().Int("client-id", 1, "Client ID (tenant)")
	marginBuyCmd.Flags().Bool("live", false, "Execute REAL orders (default is dry-run)")
	marginBuyCmd.Flags().Bool("confirm", false, "Confirm risk acknowledgement for live execution")
	marginBuyCmd.MarkFlagRequired("capital")

	// Register commands
	rootCmd.AddCommand(marginStatusCmd)
	rootCmd.AddCommand(marginPositionsCmd)
	rootCmd.AddCommand(operationsCmd)
	rootCmd.AddCommand(marginBuyCmd)
}

// invokeDjangoStatus invokes the Django status command
func invokeDjangoStatus(clientID int, detailed, useJSON bool) error {
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found")
	}

	args := []string{
		managePy,
		"status",
		"--client-id", strconv.Itoa(clientID),
	}

	if detailed {
		args = append(args, "--detailed")
	}

	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

// invokeDjangoPositions invokes the Django positions command
func invokeDjangoPositions(clientID int, live, all bool, symbol string, useJSON bool) error {
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found")
	}

	args := []string{
		managePy,
		"positions",
		"--client-id", strconv.Itoa(clientID),
	}

	if live {
		args = append(args, "--live")
	}
	if all {
		args = append(args, "--all")
	}
	if symbol != "" {
		args = append(args, "--symbol", symbol)
	}
	if useJSON {
		args = append(args, "--json")
	}

	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

// invokeDjangoOperations invokes the Django operations command
func invokeDjangoOperations(clientID int, showOpen, showClosed bool, operationID string, limit int, useJSON bool) error {
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found")
	}

	args := []string{
		managePy,
		"operations",
		"--client-id", strconv.Itoa(clientID),
		"--limit", strconv.Itoa(limit),
	}

	if showOpen {
		args = append(args, "--open")
	}
	if showClosed {
		args = append(args, "--closed")
	}
	if operationID != "" {
		args = append(args, "--id", operationID)
	}
	if useJSON {
		args = append(args, "--json")
	}

	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

// invokeDjangoMarginBuy invokes the Django isolated_margin_buy command
func invokeDjangoMarginBuy(capital, stopPercent, stopPrice string, leverage int, symbol string, clientID int, live, confirm bool) error {
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found")
	}

	args := []string{
		managePy,
		"isolated_margin_buy",
		"--capital", capital,
		"--leverage", strconv.Itoa(leverage),
		"--symbol", symbol,
		"--client-id", strconv.Itoa(clientID),
	}

	if stopPrice != "" {
		args = append(args, "--stop-price", stopPrice)
	} else {
		args = append(args, "--stop-percent", stopPercent)
	}

	if live {
		args = append(args, "--live")
	}
	if confirm {
		args = append(args, "--confirm")
	}

	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

