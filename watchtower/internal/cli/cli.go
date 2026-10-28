// Package cli is the watchtower command line: argument parsing, exit codes,
// and writing digest.json and digest.md.
package cli

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"time"

	"github.com/soni0021/reelctl-watchtower/internal/digest"
	"github.com/soni0021/reelctl-watchtower/internal/niche"
	"github.com/soni0021/reelctl-watchtower/internal/reelctl"
)

// Exit codes.
const (
	exitOK       = 0
	exitInternal = 1
	exitUsage    = 2
)

const (
	defaultMinScore    = 10.0
	defaultMinVelocity = 0.02
	defaultReelctl     = "reelctl"
)

type config struct {
	data, out, reelctl    string
	minScore, minVelocity float64
	reelctlTimeout        time.Duration
	generatedAt           string
}

// usageError is a problem with the arguments or the data directory.
type usageError struct{ err error }

func (e usageError) Error() string { return e.err.Error() }
func (e usageError) Unwrap() error { return e.err }

// Run executes the command line in args, without the program name, and returns
// the exit code.
func Run(args []string, stdout, stderr io.Writer) int {
	return run(args, stdout, stderr, time.Now)
}

func run(args []string, stdout, stderr io.Writer, clock func() time.Time) int {
	cfg, err := parse(args, stderr, clock)
	if errors.Is(err, flag.ErrHelp) {
		return exitOK
	}
	if err == nil {
		err = execute(cfg, stdout)
	}
	if err == nil {
		return exitOK
	}
	fmt.Fprintf(stderr, "watchtower: %v\n", err)
	var usage usageError
	if errors.As(err, &usage) {
		return exitUsage
	}
	return exitInternal
}

func parse(args []string, stderr io.Writer, clock func() time.Time) (config, error) {
	if len(args) == 0 || args[0] != "run" {
		return config{}, usageError{errors.New("usage: watchtower run --data <dir> --out <dir> [--reelctl <path>] [--min-score 10] [--min-velocity 0.02] [--reelctl-timeout 5m] [--now <rfc3339>]")}
	}
	var cfg config
	var now string
	fs := flag.NewFlagSet("watchtower run", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&cfg.data, "data", "", "directory with niche.json")
	fs.StringVar(&cfg.out, "out", "", "directory for digest.json, digest.md and dna/")
	fs.StringVar(&cfg.reelctl, "reelctl", defaultReelctl, "path of the reelctl binary")
	fs.Float64Var(&cfg.minScore, "min-score", defaultMinScore, "outlier multiplier that makes a post a breakout")
	fs.Float64Var(&cfg.minVelocity, "min-velocity", defaultMinVelocity, "follower growth per day, as a share, that makes a creator surge")
	fs.DurationVar(&cfg.reelctlTimeout, "reelctl-timeout", reelctl.DefaultTimeout, "longest one reelctl run may take")
	fs.StringVar(&now, "now", "", "generated_at, RFC 3339 (default: the clock)")
	if err := fs.Parse(args[1:]); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return config{}, err
		}
		return config{}, usageError{err}
	}
	if err := check(cfg, fs.NArg()); err != nil {
		return config{}, usageError{err}
	}
	generatedAt, err := generatedAt(now, clock)
	if err != nil {
		return config{}, usageError{err}
	}
	cfg.generatedAt = generatedAt
	return cfg, nil
}

func check(cfg config, extra int) error {
	switch {
	case cfg.data == "":
		return errors.New("--data is required")
	case cfg.out == "":
		return errors.New("--out is required")
	case !finite(cfg.minScore):
		return errors.New("--min-score must be a finite number")
	case cfg.minScore <= 0:
		return errors.New("--min-score must be above 0")
	case !finite(cfg.minVelocity):
		return errors.New("--min-velocity must be a finite number")
	case cfg.minVelocity < 0:
		return errors.New("--min-velocity must not be negative")
	case cfg.reelctlTimeout <= 0:
		return errors.New("--reelctl-timeout must be above 0")
	case extra > 0:
		return errors.New("unexpected argument after the flags")
	}
	return nil
}

func finite(v float64) bool { return !math.IsNaN(v) && !math.IsInf(v, 0) }

func generatedAt(now string, clock func() time.Time) (string, error) {
	t := clock()
	if now != "" {
		parsed, err := time.Parse(time.RFC3339, now)
		if err != nil {
			return "", fmt.Errorf("--now: %w", err)
		}
		t = parsed
	}
	return t.UTC().Format(time.RFC3339), nil
}

func execute(cfg config, stdout io.Writer) error {
	n, err := niche.FileSource{Dir: cfg.data}.Load()
	if err != nil {
		return usageError{err}
	}
	opts := digest.Options{MinScore: cfg.minScore, MinVelocity: cfg.minVelocity, GeneratedAt: cfg.generatedAt, OutDir: cfg.out}
	d, err := digest.Build(n, opts, reelctl.Runner{Binary: cfg.reelctl, Timeout: cfg.reelctlTimeout})
	if err != nil {
		return err
	}
	if err := write(cfg.out, d); err != nil {
		return err
	}
	fmt.Fprintf(stdout, "watchtower: %d creators, %d surging, %d failures; wrote digest.json and digest.md\n",
		len(d.Creators), countSurging(d), len(d.Failures))
	return nil
}

func write(outDir string, d digest.Digest) error {
	data, err := d.JSON()
	if err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(outDir, "digest.json"), data, 0o644); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(outDir, "digest.md"), []byte(d.Markdown()), 0o644)
}

func countSurging(d digest.Digest) int {
	n := 0
	for _, c := range d.Creators {
		if c.Surging {
			n++
		}
	}
	return n
}
