// Package reelctl runs the reelctl binary as a process.
package reelctl

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"syscall"
	"time"
)

// MaxStderr is the most stderr bytes kept from a failed run.
const MaxStderr = 2000

// DefaultTimeout is how long one reelctl run may take when Runner.Timeout is 0.
const DefaultTimeout = 5 * time.Minute

// How a failed run ended.
const (
	// CauseExited: the process ran and exited with a non-zero code.
	CauseExited = "exited"
	// CauseNotStarted: the process could not be started at all.
	CauseNotStarted = "not_started"
	// CauseSignaled: the process was ended by a signal, such as a kill.
	CauseSignaled = "signaled"
	// CauseTimedOut: the process ran past the timeout and was killed.
	CauseTimedOut = "timed_out"
)

// Runner runs reelctl. Binary is a path, or a name looked up on PATH. Timeout
// bounds one run; zero means DefaultTimeout.
type Runner struct {
	Binary  string
	Timeout time.Duration
}

// RunError is a failed reelctl run. Cause says how it ended. ExitCode is set
// only for CauseExited, Signal only for CauseSignaled.
type RunError struct {
	Cause    string
	ExitCode int
	Signal   string
	Timeout  time.Duration
	Stderr   string
}

func (e *RunError) Error() string {
	switch e.Cause {
	case CauseNotStarted:
		return "reelctl did not start: " + e.Stderr
	case CauseSignaled:
		return fmt.Sprintf("reelctl was ended by signal %s: %s", e.Signal, e.Stderr)
	case CauseTimedOut:
		return fmt.Sprintf("reelctl timed out after %s: %s", e.Timeout, e.Stderr)
	default:
		return fmt.Sprintf("reelctl exited with code %d: %s", e.ExitCode, e.Stderr)
	}
}

// waitDelay is how long after a kill the run waits for stderr to close. A
// child of reelctl can hold the pipe open past the kill.
const waitDelay = time.Second

// Decompile runs `reelctl decompile --input <input> --out <output>`.
func (r Runner) Decompile(input, output string) error {
	timeout := r.Timeout
	if timeout <= 0 {
		timeout = DefaultTimeout
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	var stderr cappedBuffer
	cmd := exec.CommandContext(ctx, r.Binary, "decompile", "--input", input, "--out", output)
	cmd.Stderr = &stderr
	cmd.WaitDelay = waitDelay
	err := cmd.Run()
	if err == nil {
		return nil
	}
	runErr := &RunError{Stderr: keep(stderr.String(), err), Timeout: timeout}
	switch {
	case errors.Is(ctx.Err(), context.DeadlineExceeded):
		runErr.Cause = CauseTimedOut
	default:
		classify(runErr, cmd, err)
	}
	return runErr
}

// classify sets how a run that was not cut off by the timeout ended.
func classify(runErr *RunError, cmd *exec.Cmd, err error) {
	var exitErr *exec.ExitError
	if !errors.As(err, &exitErr) {
		if cmd.ProcessState == nil {
			runErr.Cause = CauseNotStarted
			return
		}
		runErr.Cause, runErr.ExitCode = CauseExited, cmd.ProcessState.ExitCode()
		return
	}
	if status, ok := exitErr.Sys().(syscall.WaitStatus); ok && status.Signaled() {
		runErr.Cause, runErr.Signal = CauseSignaled, status.Signal().String()
		return
	}
	runErr.Cause, runErr.ExitCode = CauseExited, exitErr.ExitCode()
}

// cappedBuffer keeps the first MaxStderr bytes written and drops the rest. It
// accepts every write, so the process never sees a broken pipe.
type cappedBuffer struct{ buf bytes.Buffer }

func (b *cappedBuffer) Write(p []byte) (int, error) {
	if room := MaxStderr - b.buf.Len(); room > 0 {
		b.buf.Write(p[:min(room, len(p))])
	}
	return len(p), nil
}

func (b *cappedBuffer) Len() int       { return b.buf.Len() }
func (b *cappedBuffer) String() string { return b.buf.String() }

func keep(stderr string, err error) string {
	text := strings.TrimSpace(stderr)
	if text == "" {
		text = err.Error()
	}
	return strings.ToValidUTF8(text, "")
}
