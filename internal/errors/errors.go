package errors

import (
	"fmt"
	"errors"
)

// An error message that should be printed specially by the Koca CLI.
type KocaError struct {
	err error
}

func (e *KocaError) Error() string {
	return e.err.Error()
}

func (e *KocaError) Unwrap() error {
	return errors.Unwrap(e.err)
}

// Create a new [KocaError], accepting the same arguments as [fmt.Errof].
func Errorf(format string, a ...any) KocaError {
	return KocaError{err: fmt.Errorf(format, a...)}
}
