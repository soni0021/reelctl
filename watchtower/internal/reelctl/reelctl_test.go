package reelctl

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeScript(t *testing.T, body string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "reelctl")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
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
	err := (Runner{Binary: bin}).Decompile("in.mp4", "out.json")
	var runErr *RunError
	if !errors.As(err, &runErr) {
		t.Fatalf("got %T %v, want *RunError", err, err)
	}
	if runErr.ExitCode != 3 || runErr.Stderr != "error: decode failed" {
		t.Fatalf("got %+v", runErr)
	}
}

func TestDecompileReportsABinaryThatCannotStart(t *testing.T) {
	err := (Runner{Binary: filepath.Join(t.TempDir(), "absent")}).Decompile("in.mp4", "out.json")
	var runErr *RunError
	if !errors.As(err, &runErr) || runErr.ExitCode != -1 || runErr.Stderr == "" {
		t.Fatalf("got %v", err)
	}
}

func TestDecompileCapsLongStderr(t *testing.T) {
	bin := writeScript(t, "head -c 5000 /dev/zero | tr '\\0' x >&2\nexit 1")
	err := (Runner{Binary: bin}).Decompile("in.mp4", "out.json")
	var runErr *RunError
	if !errors.As(err, &runErr) || len(runErr.Stderr) != MaxStderr {
		t.Fatalf("got %v", err)
	}
	if !strings.HasPrefix(runErr.Stderr, "xxx") {
		t.Fatalf("stderr: %q", runErr.Stderr[:10])
	}
}
