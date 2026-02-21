package errors

import (
	"errors"
	"fmt"
)

// Code is a stable, machine-readable error type mapped to process exit codes.
type Code int

const (
	CodeSuccess       Code = 0
	CodeInternal      Code = 1
	CodeUsage         Code = 2
	CodeAuth          Code = 10
	CodeRateLimited   Code = 11
	CodeUnavailable   Code = 12
	CodeUnsupported   Code = 13
	CodeStale         Code = 14
	CodePartialStrict Code = 15
	CodeBlocked       Code = 16
)

// Error is a typed CLI error that carries a stable error code.
type Error struct {
	Code    Code
	Message string
	Cause   error
}

func (e *Error) Error() string {
	if e.Cause == nil {
		return e.Message
	}
	return fmt.Sprintf("%s: %v", e.Message, e.Cause)
}

func (e *Error) Unwrap() error { return e.Cause }

func New(code Code, message string) *Error {
	return &Error{Code: code, Message: message}
}

func Wrap(code Code, message string, cause error) *Error {
	return &Error{Code: code, Message: message, Cause: cause}
}

func As(err error) (*Error, bool) {
	var target *Error
	if errors.As(err, &target) {
		return target, true
	}
	return nil, false
}

func ExitCode(err error) int {
	if err == nil {
		return int(CodeSuccess)
	}
	if cliErr, ok := As(err); ok {
		return int(cliErr.Code)
	}
	return int(CodeInternal)
}
