// Package reelctl runs the reelctl binary as a process.
package reelctl

import (
	"bytes"
	"errors"
	"fmt"
	"os/exec"
	"strings"
)

// MaxStderr is the most stderr bytes kept from a failed run.
const MaxStderr = 2000

// Runner runs reelctl. Binary is a path, or a name looked up on PATH.
type Runner struct {
	Binary string
}

// RunError is a failed reelctl run. ExitCode is -1 when the process did not start.
type RunError struct {
	ExitCode int
	Stderr   string
}

func (e *RunError) Error() string {
	return fmt.Sprintf("reelctl exited with code %d: %s", e.ExitCode, e.Stderr)
}

// Decompile runs `reelctl decompile --input <input> --out <output>`.
func (r Runner) Decompile(input, output string) error {
	var stderr bytes.Buffer
	cmd := exec.Command(r.Binary, "decompile", "--input", input, "--out", output)
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err == nil {
		return nil
	}
	return &RunError{ExitCode: exitCode(err), Stderr: keep(stderr.String(), err)}
}

func exitCode(err error) int {
	var exitErr *exec.ExitError
	if errors.As(err, &exitErr) {
		return exitErr.ExitCode()
	}
	return -1
}

func keep(stderr string, err error) string {
	text := strings.TrimSpace(stderr)
	if text == "" {
		text = err.Error()
	}
	if len(text) > MaxStderr {
		text = strings.ToValidUTF8(text[:MaxStderr], "")
	}
	return text
}
