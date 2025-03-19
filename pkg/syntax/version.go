package syntax

import (
	"fmt"
)

// A parsed package version.
type Version struct {
	// The package version itself.
	PkgVer string
	// The package release.
	PkgRel int
	// The package epoch. Defaults to `0` if none is specified.
	Epoch int
}

// Get the printable representation of a [Version].
func (v Version) String() string {
	if v.Epoch == 0 {
		return fmt.Sprintf("%s-%d", v.PkgVer, v.PkgRel)
	}

	return fmt.Sprintf("%d:%s-%d", v.Epoch, v.PkgVer, v.PkgRel)
}
