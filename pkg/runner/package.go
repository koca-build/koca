package runner

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/goreleaser/nfpm/v2"
	"github.com/goreleaser/nfpm/v2/deb"
	nfpmFiles "github.com/goreleaser/nfpm/v2/files"
	"github.com/goreleaser/nfpm/v2/rpm"
	"github.com/koca-build/koca/internal/env"
	"github.com/koca-build/koca/pkg/syntax"
	shInterp "mvdan.cc/sh/v3/interp"
	shSyntax "mvdan.cc/sh/v3/syntax"
)

// The Koca package directory.
const packageDir = "koca/pkg"

// Create a Debian package file from the provided package information.
func createDebPackage(packageInfo *nfpm.Info) error {
	debName := deb.Default.ConventionalFileName(packageInfo)
	fmt.Println("Creating deb file:", debName)

	debFile, err := os.Create(debName)
	if err != nil {
		return fmt.Errorf("failed to create deb file '%s': %w", debName, err)
	}
	defer debFile.Close()

	if err := deb.Default.Package(packageInfo, debFile); err != nil {
		return fmt.Errorf("failed to write deb file '%s': %w", debName, err)
	}

	return nil
}

// Create an RPM package file from the provided package information.
func createRpmPackage(packageInfo *nfpm.Info) error {
	rpmName := rpm.Default.ConventionalFileName(packageInfo)
	fmt.Println("Creating rpm file:", rpmName)

	rpmFile, err := os.Create(rpmName)
	if err != nil {
		return fmt.Errorf("failed to create rpm file '%s': %w", rpmName, err)
	}
	defer rpmFile.Close()

	if err := rpm.Default.Package(packageInfo, rpmFile); err != nil {
		return fmt.Errorf("failed to write rpm file '%s': %w", rpmName, err)
	}

	return nil
}

// Run a packaging session from the given function.
func RunPackage(packageFunc *shSyntax.FuncDecl, vars *syntax.KocaVars, maintainer string, outputType string) error {
	absPackageDir, err := filepath.Abs(packageDir)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of '%s': %w", packageDir, err)
	}
	absBuildDir, err := filepath.Abs(buildDir)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of '%s': %w", buildDir, err)
	}

	// Make sure the package directory exists, and change directories into it.
	if err := os.MkdirAll(absPackageDir, 0755); err != nil {
		return fmt.Errorf("failed to create '%s/' directory: %w", packageDir, err)
	}

	// Set up interpreter options.
	var interpOpts []shInterp.RunnerOption
	interpOpts = append(interpOpts, shInterp.Dir(absBuildDir))
	interpOpts = append(interpOpts, shInterp.StdIO(os.Stdin, os.Stdout, os.Stderr))
	interpOpts = append(interpOpts, shInterp.Env(env.GetEnviron(
		vars,
		fmt.Sprintf("pkgdir=%s", absPackageDir),
	)))

	// Run the packaging commands.
	runner, err := shInterp.New(interpOpts...)
	if err != nil {
		return fmt.Errorf("failed to create koca interpreter: %w", err)
	}

	err = runner.Run(context.TODO(), packageFunc.Body)

	if err != nil {
		return err
	}

	var files nfpmFiles.Contents
	// [filepath.WalkDir] only returns an error if we return one, so need to handle its errors.
	filepath.WalkDir(absPackageDir, func(path string, d os.DirEntry, err error) error {
		if d.IsDir() {
			return nil
		}

		outputPath := strings.TrimPrefix(path, absPackageDir)
		files = append(files, &nfpmFiles.Content{
			Source:      path,
			Destination: outputPath,
		})

		return nil
	})

	packageInfo := nfpm.Info{
		Name:        vars.PkgName,
		Version:     vars.Version.String(),
		Description: vars.PkgDesc,
		Platform:    "linux",
		Section:     "default",
		Arch:        vars.Archs[0].String(),
		Maintainer:  maintainer,
		Overridables: nfpm.Overridables{
			Deb: nfpm.Deb{
				Arch: vars.Archs[0].DebianString(),
			},
			RPM: nfpm.RPM{
				Arch: vars.Archs[0].String(),
			},
			Contents: files,
		},
	}

	if outputType == "deb" {
		return createDebPackage(&packageInfo)
	} else if outputType == "rpm" {
		return createRpmPackage(&packageInfo)
	}
	return fmt.Errorf("unknown output type '%s'", outputType)
}
