package runner

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/koca-build/koca/internal/env"
	"github.com/koca-build/koca/pkg/syntax"
	shInterp "mvdan.cc/sh/v3/interp"
	shSyntax "mvdan.cc/sh/v3/syntax"
)

// The Koca build directory.
const buildDir = "koca/src"

// Run a build session from the given function.
func RunBuild(buildFunc *shSyntax.FuncDecl, vars *syntax.KocaVars) error {
	absBuildDir, err := filepath.Abs(buildDir)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of '%s': %w", buildDir, err)
	}

	// Make sure the build directory exists, and change directories into it.
	if err := os.MkdirAll(absBuildDir, 0755); err != nil {
		return fmt.Errorf("failed to create '%s/' directory: %w", buildDir, err)
	}

	// Set up interpreter options.
	var interpOpts []shInterp.RunnerOption
	interpOpts = append(interpOpts, shInterp.Dir(absBuildDir))
	interpOpts = append(interpOpts, shInterp.StdIO(os.Stdin, os.Stdout, os.Stderr))
	interpOpts = append(interpOpts, shInterp.Env(env.GetEnviron(vars)))

	// Run the build commands.
	runner, err := shInterp.New(interpOpts...)
	if err != nil {
		return fmt.Errorf("failed to create koca interpreter: %w", err)
	}

	err = runner.Run(context.TODO(), buildFunc.Body)

	if err != nil {
		return err
	}

	return nil
}
