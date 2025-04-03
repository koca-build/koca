package syntax

import "fmt"

// Valid architectures.
type Arch int

const (
	// Architecture-independent packages. The resulting executables can be ran on any architecture, without re-compiling.
	All Arch = iota
	// Architecture-dependent packages. The package can be built for any architecture, but the resulting executables are architecture-specific.
	Any
	// The `x86_64` architecture.
	X86_64
)

// Convert a string to an architecture.
func ParseArch(arch string) (Arch, error) {
	switch arch {
	case "all":
		return All, nil
	case "any":
		return Any, nil
	case "x86_64":
		return X86_64, nil
	default:
		return 0, fmt.Errorf("unknown architecture: %s", arch)
	}
}

// Get the printable representation of an [Arch].
func (a Arch) String() string {
	switch a {
	case All:
		return "all"
	case Any:
		return "any"
	case X86_64:
		return "x86_64"
	default:
		panic(fmt.Sprintf("unknown architecture: %d", a))
	}
}

// Get the Debian version of an [Arch].
func (a Arch) DebianString() string {
	switch a {
	case All:
		return "all"
	case Any:
		return "any"
	case X86_64:
		return "amd64"
	default:
		panic(fmt.Sprintf("unknown architecture: %d", a))
	}
}
