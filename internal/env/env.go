package env

import (
	"fmt"
	"os"

	"github.com/koca-build/koca/pkg/syntax"
	shExpand "mvdan.cc/sh/v3/expand"
)

// Get an [shExpand.Environ] implementation with Koca environment variables incorporated. `extraVars` is an optional string-separated list of extra environment variables to inject (i.e. `key=value`).
func GetEnviron(vars *syntax.KocaVars, extraVars ...string) shExpand.Environ {
	envVars := os.Environ()
	envVars = append(envVars, fmt.Sprintf("pkgname=%s", vars.PkgName))
	envVars = append(envVars, fmt.Sprintf("pkgver=%s", vars.Version.PkgVer))
	envVars = append(envVars, fmt.Sprintf("pkgrel=%d", vars.Version.PkgRel))
	envVars = append(envVars, fmt.Sprintf("pkgdesc=%s", vars.PkgDesc))

	if vars.Version.Epoch != 0 {
		envVars = append(envVars, fmt.Sprintf("epoch=%d", vars.Version.Epoch))
	}

	envVars = append(envVars, extraVars...)

	return shExpand.ListEnviron(envVars...)
}
