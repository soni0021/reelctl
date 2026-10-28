// Command watchtower writes a daily digest of the creators surging in a niche.
package main

import (
	"os"

	"github.com/soni0021/reelctl-watchtower/internal/cli"
)

func main() {
	os.Exit(cli.Run(os.Args[1:], os.Stdout, os.Stderr))
}
