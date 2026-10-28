package reelctl

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func writeScript(t *testing.T, body string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "reelctl")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

func runError(t *testing.T, err error) *RunError {
	t.Helper()
	var runErr *RunError
	if !errors.As(err, &runErr) {
		t.Fatalf("got %T %v, want *RunError", err, err)
	}
	return runErr
}

func TestDecompilePassesTheDocumentedArguments(t *testing.T) {
	out := filepath.Join(t.TempDir(), "out.json")
	bin := writeScript(t, `[ "$1" = decompile ] && [ "$2" = --input ] && [ "$3" = in.mp4 ] && [ "$4" = --out ] || exit 9
echo "{}" > "$5"`)
	if err := (Runner{Binary: bin}).Decompile("in.mp4", out); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(out); err != nil {
		t.Fatalf("the output was not written: %v", err)
	}
}

func TestDecompileKeepsTheExitCodeAndStderr(t *testing.T) {
	bin := writeScript(t, "echo 'error: decode failed' >&2\nexit 3")
	runErr := runError(t, (Runner{Binary: bin}).Decompile("in.mp4", "out.json"))
	if runErr.Cause != CauseExited || runErr.ExitCode != 3 || runErr.Stderr != "error: decode failed" {
		t.Fatalf("got %+v", runErr)
	}
}

func TestDecompileReportsABinaryThatCannotStart(t *testing.T) {
	runErr := runError(t, (Runner{Binary: filepath.Join(t.TempDir(), "absent")}).Decompile("in.mp4", "out.json"))
	if runErr.Cause != CauseNotStarted || runErr.Stderr == "" {
		t.Fatalf("got %+v", runErr)
	}
}

func TestAKilledProcessIsASignalNotAFailureToStart(t *testing.T) {
	bin := writeScript(t, "echo partial >&2\nkill -9 $$")
	runErr := runError(t, (Runner{Binary: bin}).Decompile("in.mp4", "out.json"))
	if runErr.Cause != CauseSignaled || runErr.Signal != "killed" {
		t.Fatalf("got %+v", runErr)
	}
	if !strings.Contains(runErr.Error(), "killed") || strings.Contains(runErr.Error(), "did not start") {
		t.Fatalf("message %q", runErr.Error())
	}
}

func TestDecompileCapsLongStderr(t *testing.T) {
	bin := writeScript(t, "head -c 5000 /dev/zero | tr '\\0' x >&2\nexit 1")
	runErr := runError(t, (Runner{Binary: bin}).Decompile("in.mp4", "out.json"))
	if len(runErr.Stderr) != MaxStderr {
		t.Fatalf("got %d bytes", len(runErr.Stderr))
	}
	if !strings.HasPrefix(runErr.Stderr, "xxx") {
		t.Fatalf("stderr: %q", runErr.Stderr[:10])
	}
}

func TestTheStderrBufferStopsGrowingAtTheCap(t *testing.T) {
	var buf cappedBuffer
	chunk := bytes.Repeat([]byte("y"), 64<<10)
	for i := 0; i < 160; i++ {
		n, err := buf.Write(chunk)
		if n != len(chunk) || err != nil {
			t.Fatalf("write %d returned %d, %v; a capped writer must accept everything", i, n, err)
		}
	}
	if got := buf.Len(); got != MaxStderr {
		t.Fatalf("kept %d bytes, want %d", got, MaxStderr)
	}
}

func TestASlowReelctlIsStoppedAtTheTimeout(t *testing.T) {
	// No exec: the sleep is a child that keeps stderr open after the shell is killed.
	bin := writeScript(t, "echo started >&2\nsleep 6\n")
	begin := time.Now()
	runErr := runError(t, (Runner{Binary: bin, Timeout: time.Second}).Decompile("in.mp4", "out.json"))
	elapsed := time.Since(begin)
	if runErr.Cause != CauseTimedOut {
		t.Fatalf("got %+v", runErr)
	}
	if elapsed > 4*time.Second {
		t.Fatalf("took %v with a 1s timeout", elapsed)
	}
	if !strings.Contains(runErr.Error(), "timed out after 1s") || runErr.Stderr == "" {
		t.Fatalf("message %q stderr %q", runErr.Error(), runErr.Stderr)
	}
}

func TestAFastReelctlIsNotCutOffByTheTimeout(t *testing.T) {
	bin := writeScript(t, "exit 0")
	if err := (Runner{Binary: bin, Timeout: 30 * time.Second}).Decompile("in.mp4", "out.json"); err != nil {
		t.Fatal(err)
	}
}
