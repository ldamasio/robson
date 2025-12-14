/*
Package cmd implements all CLI subcommands for robson-go.

The root command serves as the entry point and coordinator for all subcommands.
*/
package cmd

import (
	"fmt"

	"github.com/spf13/cobra"
)

var (
	// Global flags
	jsonOutput bool

	// Version information (set by main)
	version   string
	buildTime string

	rootCmd = &cobra.Command{
		Use:   "robson-go",
		Short: "Robson - Cryptocurrency trading platform CLI",
		Long: `Robson is an open-source cryptocurrency trading platform.

This CLI provides access to trading operations, reporting, and agentic workflows
that separate planning, validation, and execution.

Design philosophy:
  - Plan before you act
  - Validate before you commit
  - Execute with intent

Legacy mode:
  The CLI maintains backward compatibility with legacy flags (--help, --buy, etc.)
  which are automatically translated by the C router.`,
		SilenceUsage:  true,
		SilenceErrors: true,
	}
)

// Execute runs the root command
func Execute() error {
	return rootCmd.Execute()
}

// SetVersionInfo sets version information for the CLI
func SetVersionInfo(v, bt string) {
	version = v
	buildTime = bt
}

func init() {
	// Global flags available to all subcommands
	rootCmd.PersistentFlags().BoolVar(&jsonOutput, "json", false, "Output in JSON format for automation/agents")

	// Add version command
	rootCmd.AddCommand(&cobra.Command{
		Use:   "version",
		Short: "Print version information",
		Run: func(cmd *cobra.Command, args []string) {
			if jsonOutput {
				fmt.Printf(`{"version":"%s","buildTime":"%s"}`+"\n", version, buildTime)
			} else {
				fmt.Printf("robson-go version %s (built %s)\n", version, buildTime)
			}
		},
	})

	// Add all subcommands
	rootCmd.AddCommand(helpCmd)
	rootCmd.AddCommand(reportCmd)
	rootCmd.AddCommand(sayCmd)
	rootCmd.AddCommand(buyCmd)
	rootCmd.AddCommand(sellCmd)
	rootCmd.AddCommand(planCmd)
	rootCmd.AddCommand(validateCmd)
	rootCmd.AddCommand(executeCmd)
}
