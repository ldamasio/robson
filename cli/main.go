/*
Package main implements the robson-go CLI.

Design philosophy:
  - CLI as a CONTRACT OF INTENT (plan → validate → execute)
  - Clear separation of concerns
  - Human-readable by default, machine-readable on demand (--json)

Architecture:
  - Root command: robson-go
  - Legacy subcommands: help, report, say, buy, sell
  - Agentic workflow: plan, validate, execute
*/
package main

import (
	"fmt"
	"os"

	"github.com/ldamasio/robson/cli/cmd"
)

var (
	// Version is set at build time
	Version = "dev"
	// BuildTime is set at build time
	BuildTime = "unknown"
)

func main() {
	// Set version info for commands
	cmd.SetVersionInfo(Version, BuildTime)

	if err := cmd.Execute(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
}
