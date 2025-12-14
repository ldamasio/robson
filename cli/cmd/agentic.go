/*
Package cmd - Agentic workflow commands

These commands implement the core philosophy:
  PLAN → VALIDATE → EXECUTE

Just as in trading we separate:
  - Idea formulation
  - Validation
  - Execution

We separate these concerns at the CLI level to prevent unintended actions.
*/
package cmd

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"time"

	"github.com/spf13/cobra"
)

// planCmd creates an execution plan
var planCmd = &cobra.Command{
	Use:   "plan <strategy> [parameters...]",
	Short: "Create an execution plan",
	Long: `Create a detailed execution plan for a trading strategy.

This is the FIRST step in the agentic workflow. The plan:
  - Defines what action will be taken
  - Specifies all parameters
  - Calculates expected outcomes
  - Generates a unique plan ID
  - Does NOT execute anything

Philosophy:
  "Plan before you act. Know what you're doing before you do it."

Examples:
  robson plan buy BTCUSDT 0.001 --limit 50000
  robson plan sell ETHUSDT 0.5 --market
  robson plan rebalance --target-allocation btc:50,eth:30,usdt:20`,
	Args: cobra.MinimumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		strategy := args[0]
		params := args[1:]

		// Generate plan ID from timestamp + strategy + params
		planData := fmt.Sprintf("%d-%s-%v", time.Now().Unix(), strategy, params)
		hash := sha256.Sum256([]byte(planData))
		planID := hex.EncodeToString(hash[:])[:16]

		plan := map[string]interface{}{
			"planID":    planID,
			"strategy":  strategy,
			"params":    params,
			"createdAt": time.Now().Format(time.RFC3339),
			"status":    "draft",
			"validated": false,
		}

		if jsonOutput {
			return outputJSON(plan)
		}

		fmt.Println("╔════════════════════════════════════════════════════════════╗")
		fmt.Println("║                    EXECUTION PLAN                         ║")
		fmt.Println("╚════════════════════════════════════════════════════════════╝")
		fmt.Println()
		fmt.Printf("Plan ID:    %s\n", planID)
		fmt.Printf("Strategy:   %s\n", strategy)
		fmt.Printf("Parameters: %v\n", params)
		fmt.Printf("Created:    %s\n", time.Now().Format("2006-01-02 15:04:05"))
		fmt.Printf("Status:     DRAFT (not validated)\n")
		fmt.Println()
		fmt.Println("NEXT STEPS:")
		fmt.Println("  1. Review this plan carefully")
		fmt.Println("  2. Validate it: robson validate", planID)
		fmt.Println("  3. If valid, execute: robson execute", planID)
		fmt.Println()
		fmt.Println("⚠️  This plan has NOT been executed. It's just a blueprint.")
		fmt.Println()

		return nil
	},
}

// validateCmd validates an execution plan
var validateCmd = &cobra.Command{
	Use:   "validate <plan-id> --client-id <id> [options]",
	Short: "Validate an execution plan",
	Long: `Validate an execution plan before execution.

This is the SECOND step in the agentic workflow (PAPER TRADING stage).

Validation performs operational and financial checks:
  - Tenant isolation (client_id is mandatory)
  - Risk configuration (drawdown, stop-loss, position sizing)
  - Operation parameters (symbol, quantity, price)
  - Does NOT execute anything

Philosophy:
  "Validate before you commit. Catch errors before they cost money."

This is NOT developer CI. This is operational and financial validation.

Examples:
  robson validate abc123 --client-id 1 --strategy-id 5
  robson validate abc123 --client-id 1 --operation-type buy --symbol BTCUSDT --quantity 0.001 --price 50000
  robson validate abc123 --client-id 1 --json`,
	Args: cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		planID := args[0]

		// Get flags
		clientID, _ := cmd.Flags().GetInt("client-id")
		strategyID, _ := cmd.Flags().GetInt("strategy-id")
		opType, _ := cmd.Flags().GetString("operation-type")
		symbol, _ := cmd.Flags().GetString("symbol")
		quantity, _ := cmd.Flags().GetString("quantity")
		price, _ := cmd.Flags().GetString("price")

		// Invoke Django management command
		return invokeDjangoValidation(planID, clientID, strategyID, opType, symbol, quantity, price, jsonOutput)
	},
}

func init() {
	// Add flags to validate command
	validateCmd.Flags().Int("client-id", 0, "Client ID (tenant) - MANDATORY for tenant isolation")
	validateCmd.Flags().Int("strategy-id", 0, "Strategy ID to load risk configuration from")
	validateCmd.Flags().String("operation-type", "", "Operation type (buy, sell, cancel)")
	validateCmd.Flags().String("symbol", "", "Trading symbol (e.g., BTCUSDT)")
	validateCmd.Flags().String("quantity", "", "Order quantity")
	validateCmd.Flags().String("price", "", "Order price (for limit orders)")

	// Mark client-id as required
	validateCmd.MarkFlagRequired("client-id")
}

// executeCmd executes a plan (DRY-RUN by default, LIVE requires explicit flags)
var executeCmd = &cobra.Command{
	Use:   "execute <plan-id> --client-id <id> [options]",
	Short: "Execute a plan (DRY-RUN by default)",
	Long: `Execute a plan with SAFE BY DEFAULT semantics.

This is the FINAL step in the agentic workflow: PLAN → VALIDATE → EXECUTE

SAFE BY DEFAULT:
  - DRY-RUN is the default (simulation, no real orders)
  - LIVE requires --live AND --acknowledge-risk flags
  - LIVE requires prior validation
  - All executions are audited

DRY-RUN Mode (default):
  - Simulates execution
  - No real orders placed
  - Always allowed
  - Useful for testing and verification

LIVE Mode (requires explicit acknowledgement):
  - Places REAL orders on the exchange
  - Requires --live flag
  - Requires --acknowledge-risk flag
  - Requires prior validation
  - Enforces execution limits

Philosophy:
  "Execute with intent. Safety first, always."

Examples:
  # DRY-RUN (default, safe)
  robson execute abc123 --client-id 1

  # LIVE (requires explicit acknowledgement)
  robson execute abc123 --client-id 1 --live --acknowledge-risk

  # With strategy limits
  robson execute abc123 --client-id 1 --strategy-id 5 --live --acknowledge-risk`,
	Args: cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		planID := args[0]

		// Get flags
		clientID, _ := cmd.Flags().GetInt("client-id")
		strategyID, _ := cmd.Flags().GetInt("strategy-id")
		opType, _ := cmd.Flags().GetString("operation-type")
		symbol, _ := cmd.Flags().GetString("symbol")
		quantity, _ := cmd.Flags().GetString("quantity")
		price, _ := cmd.Flags().GetString("price")
		live, _ := cmd.Flags().GetBool("live")
		acknowledgeRisk, _ := cmd.Flags().GetBool("acknowledge-risk")
		validated, _ := cmd.Flags().GetBool("validated")
		validationPassed, _ := cmd.Flags().GetBool("validation-passed")

		// Invoke Django execution
		return invokeDjangoExecution(
			planID, clientID, strategyID,
			opType, symbol, quantity, price,
			live, acknowledgeRisk, validated, validationPassed,
			jsonOutput,
		)
	},
}

func init() {
	// Add flags to execute command
	executeCmd.Flags().Int("client-id", 0, "Client ID (tenant) - MANDATORY")
	executeCmd.Flags().Int("strategy-id", 0, "Strategy ID for limits and configuration")
	executeCmd.Flags().String("operation-type", "", "Operation type (buy, sell, cancel)")
	executeCmd.Flags().String("symbol", "", "Trading symbol (e.g., BTCUSDT)")
	executeCmd.Flags().String("quantity", "", "Order quantity")
	executeCmd.Flags().String("price", "", "Order price (for limit orders)")
	executeCmd.Flags().Bool("live", false, "LIVE mode (real orders) - requires --acknowledge-risk")
	executeCmd.Flags().Bool("acknowledge-risk", false, "Acknowledge risk of LIVE execution (REQUIRED for --live)")
	executeCmd.Flags().Bool("validated", false, "Mark as validated (set by validation step)")
	executeCmd.Flags().Bool("validation-passed", false, "Mark validation as passed (set by validation step)")

	// Mark client-id as required
	executeCmd.MarkFlagRequired("client-id")
}

// invokeDjangoValidation invokes the Django management command for validation
func invokeDjangoValidation(planID string, clientID, strategyID int, opType, symbol, quantity, price string, useJSON bool) error {
	// Find Django manage.py
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found. Make sure you're running from the robson repository root")
	}

	// Build command
	args := []string{
		managePy,
		"validate_plan",
		"--plan-id", planID,
		"--client-id", strconv.Itoa(clientID),
	}

	// Add optional arguments
	if strategyID > 0 {
		args = append(args, "--strategy-id", strconv.Itoa(strategyID))
	}
	if opType != "" {
		args = append(args, "--operation-type", opType)
	}
	if symbol != "" {
		args = append(args, "--symbol", symbol)
	}
	if quantity != "" {
		args = append(args, "--quantity", quantity)
	}
	if price != "" {
		args = append(args, "--price", price)
	}
	if useJSON {
		args = append(args, "--json")
	}

	// Execute Django command
	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	err := cmd.Run()
	if err != nil {
		// Exit code 1 = validation failed (expected)
		// Other errors = actual command failure
		if exitErr, ok := err.(*exec.ExitError); ok {
			if exitErr.ExitCode() == 1 {
				// Validation failed (Django already printed the report)
				return fmt.Errorf("validation failed")
			}
		}
		return fmt.Errorf("failed to execute Django validation: %w", err)
	}

	return nil
}

// findDjangoManagePy finds the Django manage.py file
func findDjangoManagePy() string {
	// Try common locations
	candidates := []string{
		"apps/backend/monolith/manage.py",
		"../apps/backend/monolith/manage.py",
		"../../apps/backend/monolith/manage.py",
	}

	for _, path := range candidates {
		if _, err := os.Stat(path); err == nil {
			return path
		}
	}

	return ""
}

// invokeDjangoExecution invokes the Django management command for execution
func invokeDjangoExecution(
	planID string,
	clientID, strategyID int,
	opType, symbol, quantity, price string,
	live, acknowledgeRisk, validated, validationPassed bool,
	useJSON bool,
) error {
	// Find Django manage.py
	managePy := findDjangoManagePy()
	if managePy == "" {
		return fmt.Errorf("Django manage.py not found. Make sure you're running from the robson repository root")
	}

	// Build command
	args := []string{
		managePy,
		"execute_plan",
		"--plan-id", planID,
		"--client-id", strconv.Itoa(clientID),
	}

	// Add optional arguments
	if strategyID > 0 {
		args = append(args, "--strategy-id", strconv.Itoa(strategyID))
	}
	if opType != "" {
		args = append(args, "--operation-type", opType)
	}
	if symbol != "" {
		args = append(args, "--symbol", symbol)
	}
	if quantity != "" {
		args = append(args, "--quantity", quantity)
	}
	if price != "" {
		args = append(args, "--price", price)
	}
	if live {
		args = append(args, "--live")
	}
	if acknowledgeRisk {
		args = append(args, "--acknowledge-risk")
	}
	if validated {
		args = append(args, "--validated")
	}
	if validationPassed {
		args = append(args, "--validation-passed")
	}
	if useJSON {
		args = append(args, "--json")
	}

	// Execute Django command
	cmd := exec.Command("python", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	err := cmd.Run()
	if err != nil {
		// Exit code 1 = execution failed or blocked
		if exitErr, ok := err.(*exec.ExitError); ok {
			if exitErr.ExitCode() == 1 {
				// Execution blocked or failed (Django already printed the report)
				return fmt.Errorf("execution blocked or failed")
			}
		}
		return fmt.Errorf("failed to execute Django command: %w", err)
	}

	return nil
}
