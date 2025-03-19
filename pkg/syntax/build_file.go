package syntax

import (
	"fmt"
	"io"
	"net/mail"
	"strconv"
	"strings"

	"github.com/koca-build/koca/pkg/syntax/literals"
	"github.com/samber/lo"
	shExpand "mvdan.cc/sh/v3/expand"
	shSyntax "mvdan.cc/sh/v3/syntax"
)

// The variables from a Koca build file.
type KocaVars struct {
	// The package name.
	PkgName string
	// The package version.
	Version Version
	// The package description.
	PkgDesc string
	// The package's target architectures.
	Archs []Arch
}

// The functions from a Koca build file.
type KocaFuncs struct {
	// The build function.
	BuildFunc *shSyntax.FuncDecl
	// The package function.
	PackageFunc *shSyntax.FuncDecl
}

// A parsed Koca build file.
type BuildFile struct {
	// The maintainer.
	Maintainer *mail.Address
	// Variables.
	Vars KocaVars
	// Functions.
	Funcs KocaFuncs
}

// A maintainer comment line.
var maintainerComment = "Maintainer: "

// Parse the maintainer field.
func parseMaintainer(file *shSyntax.File) (*mail.Address, error) {
	var maintainerString string
	for _, comment := range file.Stmts[0].Comments {
		strippedString := strings.TrimSpace(comment.Text)

		if strings.HasPrefix(strippedString, maintainerComment) {
			if len(maintainerString) != 0 {
				return nil, fmt.Errorf("only one maintainer is allowed")
			}

			maintainerString = strings.TrimPrefix(strippedString, maintainerComment)
		}
	}

	address, err := mail.ParseAddress(maintainerString)
	if err != nil {
		return nil, fmt.Errorf("maintainer parsing error: %w", err)
	}

	return address, nil
}

// Get the needed variables for a Koca build file. The `error` slice is `nil` unless errors were found.
func parseKocaVars(file *shSyntax.File) (KocaVars, []error) {
	var errorList []error

	var pkgName string
	var pkgVer string
	var pkgRel string
	var epoch string
	var pkgDesc string
	var archs []string

	_, _, _, _ = pkgVer, pkgRel, epoch, archs

	// Parse out the raw data.
	for _, stmt := range file.Stmts {
		callExpr, ok := stmt.Cmd.(*shSyntax.CallExpr)
		if !ok {
			continue
		}

		getLine := func() uint {
			return callExpr.Pos().Line()
		}

		if len(callExpr.Args) != 0 {
			errorList = append(errorList, fmt.Errorf("top-level command execution disallowed (line %d)", getLine()))
			continue
		}

		if len(callExpr.Assigns) != 1 {
			errorList = append(errorList, fmt.Errorf("assign statements are limited to 1 per line (line %d)", getLine()))
			continue
		}

		assignment := callExpr.Assigns[0]

		// Get the name and value.
		name := assignment.Name.Value
		getValue := func() (string, error) {
			value, err := shExpand.Literal(nil, assignment.Value)
			if err != nil {
				return "", fmt.Errorf("failed to parse variable value for '%s': %w (line %d)", name, err, getLine())
			}
			return value, nil
		}

		// Parse out based on the variable type.
		var err error

		switch name {
		case literals.PkgName:
			pkgName, err = getValue()
			if err != nil {
				errorList = append(errorList, err)
			}

		case literals.PkgVer:
			pkgVer, err = getValue()
			if err != nil {
				errorList = append(errorList, err)
			}

		case literals.PkgRel:
			pkgRel, err = getValue()
			if err != nil {
				errorList = append(errorList, err)
			}

		case literals.Epoch:
			epoch, err = getValue()
			if err != nil {
				errorList = append(errorList, err)
			}

		case literals.PkgDesc:
			pkgDesc, err = getValue()
			if err != nil {
				errorList = append(errorList, err)
			}
		case literals.Arch:
			archs = lo.Map(assignment.Array.Elems, func(item *shSyntax.ArrayElem, index int) string {
				value, err := shExpand.Literal(nil, item.Value)
				if err != nil {
					panic(err)
				}
				return value
			})
		}
	}

	// Ensure all needed values have been filled out.
	valueChecks := []struct {
		VarName string
		Length  int
	}{
		{literals.PkgName, len(pkgName)},
		{literals.PkgVer, len(pkgVer)},
		{literals.PkgRel, len(pkgRel)},
		{literals.Arch, len(archs)},
	}

	for _, check := range valueChecks {
		if check.Length == 0 {
			errorList = append(errorList, fmt.Errorf("variable '%s' is required", check.VarName))
		}
	}

	// Ensure the pkgrel and epoch are integers.
	var intPkgRel int
	var intEpoch int

	intChecks := []struct {
		VarName string
		Value   string
		Target  *int
	}{
		{literals.PkgRel, pkgRel, &intPkgRel},
		{literals.Epoch, epoch, &intEpoch},
	}

	for _, check := range intChecks {
		if check.Value == "" {
			*check.Target = 0
			continue
		}

		value, err := strconv.Atoi(check.Value)
		if err != nil {
			errorList = append(errorList, fmt.Errorf("variable '%s' must be an integer", check.VarName))
		}

		*check.Target = value
	}

	version := Version{
		PkgVer: pkgVer,
		PkgRel: intPkgRel,
		Epoch:  intEpoch,
	}

	// Ensure the architectures are valid.
	var parsedArchList []Arch

	for _, arch := range archs {
		parsedArch, err := ParseArch(arch)
		if err != nil {
			errorList = append(errorList, err)
		}

		parsedArchList = append(parsedArchList, parsedArch)
	}

	// Return the errors or parsed build file.
	if errorList != nil {
		return KocaVars{}, errorList
	}

	return KocaVars{
		PkgName: pkgName,
		Version: version,
		PkgDesc: pkgDesc,
		Archs:   parsedArchList,
	}, nil
}

// Get the needed functions for a Koca build file. The `error` slice is `nil` unless errors were found.
func parseKocaFuncs(file *shSyntax.File) (KocaFuncs, []error) {
	var errorList []error

	var buildFunc *shSyntax.FuncDecl
	var packageFunc *shSyntax.FuncDecl

	// Parse out the raw data.
	for _, stmt := range file.Stmts {
		funcDecl, ok := stmt.Cmd.(*shSyntax.FuncDecl)
		if !ok {
			continue
		}

		switch funcDecl.Name.Value {
		case literals.BuildFunc:
			buildFunc = funcDecl
		case literals.PackageFunc:
			packageFunc = funcDecl
		default:
			errorList = append(errorList, fmt.Errorf("unknown function '%s'", funcDecl.Name.Value))
		}
	}

	if errorList != nil {
		return KocaFuncs{}, errorList
	}

	return KocaFuncs{
		BuildFunc:   buildFunc,
		PackageFunc: packageFunc,
	}, nil
}

// Parse an [io.Reader] into a new [BuildFile]. The `error` slice is `nil` unless errors were found.
func ParseBuildFile(input io.Reader) (BuildFile, []error) {
	var errorList []error

	// Parse the file itself.
	parser := shSyntax.NewParser(shSyntax.KeepComments(true))
	parsedFile, err := parser.Parse(input, "koca.bash")

	if err != nil {
		errorList = append(errorList, fmt.Errorf("shell parser error: %w", err))
		return BuildFile{}, errorList
	}

	// Parse the maintainer.
	maintainer, err := parseMaintainer(parsedFile)

	if err != nil {
		errorList = append(errorList, err)
		return BuildFile{}, errorList
	}

	// Parse out the needed variables.
	vars, errs := parseKocaVars(parsedFile)
	if errs != nil {
		errorList = append(errorList, errs...)
	}

	// Parse out the needed functions.
	funcs, errs := parseKocaFuncs(parsedFile)
	if errs != nil {
		errorList = append(errorList, errs...)
	}

	// Return the errors or parsed build file.
	if errorList != nil {
		return BuildFile{}, errorList
	}

	return BuildFile{
		Maintainer: maintainer,
		Vars:       vars,
		Funcs:      funcs,
	}, nil
}
