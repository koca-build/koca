package main

import (
	"os"

	"github.com/koca-build/koca/internal/logging"
	"github.com/koca-build/koca/pkg/runner"
	"github.com/koca-build/koca/pkg/syntax"
	"github.com/spf13/cobra"
)

var (
	buildFile string
	rootCmd   = &cobra.Command{
		Use:   "koca build-file",
		Short: "A modern, universal, and system-native package manager",
		Args:  cobra.ExactArgs(1),
		Run: func(cmd *cobra.Command, args []string) {
			ok := runBuild(args[0])

			if !ok {
				os.Exit(1)
			}
		},
	}
)

// Run a Koca session, returning if any haltable errors occurred.
func runBuild(buildFile string) bool {
	buildFileContent, err := os.Open(buildFile)
	if err != nil {
		logging.Err("failed to open build file: %w", err)
		return false
	}

	logging.Info("parsing build file: '%s'...", buildFile)

	parsedBuildFile, errs := syntax.ParseBuildFile(buildFileContent)
	if errs != nil {
		for _, err := range errs {
			logging.Err("failed to parse build file: %w", err)
		}
		return false
	}

	logging.Info("running build...")
	if err := runner.RunBuild(parsedBuildFile.Funcs.BuildFunc, &parsedBuildFile.Vars); err != nil {
		logging.Err("failed to run build: %w", err)
		return false
	}

	logging.Info("running packaging...")
	if err := runner.RunPackage(parsedBuildFile.Funcs.PackageFunc, &parsedBuildFile.Vars); err != nil {
		logging.Err("failed to run packaging: %w", err)
		return false
	}

	logging.Info("package '%s' built successfully!", parsedBuildFile.Vars.PkgName)
	return true
}

func main() {
	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}
