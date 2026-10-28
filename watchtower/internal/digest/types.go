// Package digest builds the daily digest: it scores each creator's posts,
// decompiles the posts that matter with reelctl, compares breakouts with
// baseline posts, and renders the result as JSON and Markdown.
package digest

import (
	"bytes"
	"encoding/json"

	"github.com/soni0021/reelctl-watchtower/internal/blueprint"
)

// Decompiler turns a media file into an edit_dna.json file. A *reelctl.RunError
// carries the exit code and stderr of a failed run.
type Decompiler interface {
	Decompile(input, output string) error
}

// Options are the parameters of one run.
type Options struct {
	MinScore    float64
	MinVelocity float64
	GeneratedAt string
	OutDir      string
}

// Digest is the whole of digest.json.
type Digest struct {
	GeneratedAt string    `json:"generated_at"`
	Niche       string    `json:"niche"`
	MinScore    float64   `json:"min_score"`
	MinVelocity float64   `json:"min_velocity"`
	Creators    []Creator `json:"creators"`
	Failures    []Failure `json:"failures"`
}

// Creator is one account's section of the digest.
type Creator struct {
	Handle         string               `json:"handle"`
	Surging        bool                 `json:"surging"`
	SurgingBy      []string             `json:"surging_by"`
	BestPostID     string               `json:"best_post_id,omitempty"`
	BestMultiplier *float64             `json:"best_multiplier"`
	Velocity       *Velocity            `json:"velocity"`
	Breakouts      []Breakout           `json:"breakouts"`
	BaselinePosts  int                  `json:"baseline_posts"`
	Blueprint      *blueprint.Blueprint `json:"blueprint"`
	Skipped        []Skip               `json:"skipped"`
}

// Velocity is the growth over the last 7 days of snapshots. Shares are the
// slope divided by the first value in the window.
type Velocity struct {
	Snapshots       int     `json:"snapshots"`
	FollowersPerDay float64 `json:"followers_per_day"`
	FollowersShare  float64 `json:"followers_share_per_day"`
	ViewsPerDay     float64 `json:"views_per_day"`
	ViewsShare      float64 `json:"views_share_per_day"`
}

// Breakout is a post at or above the minimum score. EditDNA is the path of its
// decompiled edit_dna.json relative to the output directory, empty when the
// post was not decompiled.
type Breakout struct {
	ID         string  `json:"id"`
	URL        string  `json:"url,omitempty"`
	PostedAt   string  `json:"posted_at"`
	Views      int64   `json:"views"`
	Multiplier float64 `json:"multiplier"`
	EditDNA    string  `json:"edit_dna,omitempty"`
}

// Skip says what could not be computed or done, and why.
type Skip struct {
	Post   string `json:"post,omitempty"`
	Reason string `json:"reason"`
	Detail string `json:"detail"`
}

// Failure is a decompile that did not produce a usable edit_dna.json.
// ExitCode is -1 when reelctl did not start, and absent for read_edit_dna.
type Failure struct {
	Creator  string `json:"creator"`
	Post     string `json:"post"`
	Stage    string `json:"stage"`
	ExitCode int    `json:"exit_code,omitempty"`
	Detail   string `json:"detail"`
}

// Reasons for a Skip.
const (
	ReasonNotEnoughHistory = "not_enough_history"
	ReasonZeroMedian       = "zero_median"
	ReasonVelocity         = "velocity_not_computed"
	ReasonNoMedia          = "no_media"
	ReasonBlueprint        = "blueprint_not_computed"
)

// Stages of a Failure.
const (
	StageDecompile = "decompile"
	StageReadDNA   = "read_edit_dna"
)

// JSON renders the digest as indented JSON ending in a newline.
func (d Digest) JSON() ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(d); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}
