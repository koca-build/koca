package logging

import (
	"fmt"

	"github.com/fatih/color"
)

// Output an informational message.
func Info(msg string, args ...interface{}) {
	header := color.CyanString("Info:")
	errChain := fmt.Errorf(msg, args...)

	fmt.Printf("%s %s\n", header, errChain.Error())
}

// Output a warning message.
func Warn(msg string, args ...interface{}) {
	header := color.YellowString("Warning:")
	errChain := fmt.Errorf(msg, args...)

	fmt.Printf("%s %s\n", header, errChain.Error())
}

// Output an error message.
func Err(msg string, args ...interface{}) {
	header := color.RedString("Error:")
	errChain := fmt.Errorf(msg, args...)

	fmt.Printf("%s %s\n", header, errChain.Error())
}
