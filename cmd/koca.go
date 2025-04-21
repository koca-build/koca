package main

import (
	"fmt"
	"os"

	"github.com/koca-build/fakeroot"
	"github.com/koca-build/koca/internal/logging"
	"github.com/koca-build/koca/pkg/runner"
	"github.com/koca-build/koca/pkg/syntax"
	"github.com/spf13/cobra"
)

var cliOpts struct {
	internalPackage bool
	outputType      string
}

var rootCmd = &cobra.Command{
	Use:   "koca build-file",
	Short: "A modern, universal, and system-native package manager",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		ok := runSession(args[0])

		if !ok {
			os.Exit(1)
		}
	},
}

func init() {
	rootCmd.Flags().BoolVar(&cliOpts.internalPackage, "internal-package", false, "[INTERNAL] start packaging stage")
	rootCmd.Flags().MarkHidden("internal-package")
	rootCmd.Flags().StringVar(&cliOpts.outputType, "output-type", "", "output type (deb, rpm)")

	rootCmd.Flags()
}

// Run a Koca session, returning if any haltable errors occurred.
func runSession(buildFile string) bool {
	buildFileContent, err := os.Open(buildFile)
	if err != nil {
		logging.Err("failed to open build file: %w", err)
		return false
	}

	if !cliOpts.internalPackage {
		logging.Info("parsing build file: '%s'...", buildFile)
	}

	parsedBuildFile, errs := syntax.ParseBuildFile(buildFileContent)
	if errs != nil {
		for _, err := range errs {
			logging.Err("failed to parse build file: %w", err)
		}
		return false
	}

	// This actually runs after the build stage below, since we trigger the internal package flag during execution.
	if cliOpts.internalPackage {
		logging.Info("running packaging...")
		maintainer := fmt.Sprintf(
			"%s <%s>",
			parsedBuildFile.Maintainer.Name,
			parsedBuildFile.Maintainer.Address,
		)
		if err := runner.RunPackage(parsedBuildFile.Funcs.PackageFunc, &parsedBuildFile.Vars, maintainer, cliOpts.outputType); err != nil {
			logging.Err("failed to run packaging: %w", err)
			return false
		}

		return true
	}

	logging.Info("running build...")
	if err := runner.RunBuild(parsedBuildFile.Funcs.BuildFunc, &parsedBuildFile.Vars); err != nil {
		logging.Err("failed to run build: %w", err)
		return false
	}

	// We need to run the packaging stage in a chroot environment so that root permissions are set in the built archive.
	os.Args = append(os.Args, "--internal-package")
	cmd, err := fakeroot.Command(os.Args[0], os.Args[1:]...)
	if err != nil {
		logging.Err("failed to create fakeroot packaging command: %w", err)
		return false
	}

	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	if err := cmd.Run(); err != nil {
		logging.Err("failed to run fakeroot packaging command: %w", err)
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
