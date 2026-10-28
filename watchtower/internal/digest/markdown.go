package digest

import (
	"fmt"
	"strings"

	"github.com/soni0021/reelctl-watchtower/internal/blueprint"
)

// Markdown renders the digest for a person. It holds the same data as the JSON.
func (d Digest) Markdown() string {
	var b strings.Builder
	b.WriteString("# Why They Grew + Exact Edit Blueprint\n\n")
	fmt.Fprintf(&b, "Niche: %s. Generated %s.\n\n", d.Niche, d.GeneratedAt)
	fmt.Fprintf(&b, "A creator is surging when a post has an outlier multiplier of at least %g, or followers grow by more than %.1f%% a day. A sentence appears only for a change of at least %.0f%%.\n\n",
		d.MinScore, d.MinVelocity*100, blueprint.ChangeThreshold*100)
	surging, other := splitBySurging(d.Creators)
	writeCreators(&b, "Surging creators", surging)
	writeCreators(&b, "Other creators", other)
	writeFailures(&b, d.Failures)
	return b.String()
}

func splitBySurging(creators []Creator) (surging, other []Creator) {
	for _, c := range creators {
		if c.Surging {
			surging = append(surging, c)
		} else {
			other = append(other, c)
		}
	}
	return surging, other
}

func writeCreators(b *strings.Builder, title string, creators []Creator) {
	fmt.Fprintf(b, "## %s\n\n", title)
	if len(creators) == 0 {
		b.WriteString("None.\n\n")
		return
	}
	for _, c := range creators {
		writeCreator(b, c)
	}
}

func writeCreator(b *strings.Builder, c Creator) {
	fmt.Fprintf(b, "### %s\n\n", c.Handle)
	if c.Surging {
		fmt.Fprintf(b, "- Surging by: %s.\n", strings.Join(c.SurgingBy, ", "))
	} else {
		b.WriteString("- Not surging.\n")
	}
	if c.BestMultiplier != nil {
		fmt.Fprintf(b, "- Best post: %s at %.2fx the median of its earlier posts.\n", c.BestPostID, *c.BestMultiplier)
	}
	if v := c.Velocity; v != nil {
		fmt.Fprintf(b, "- Followers: %+.1f a day (%.2f%% a day) over %d snapshots. Total views: %+.1f a day (%.2f%% a day).\n",
			v.FollowersPerDay, v.FollowersShare*100, v.Snapshots, v.ViewsPerDay, v.ViewsShare*100)
	}
	fmt.Fprintf(b, "- Breakouts: %s. Baseline posts: %d.\n\n", breakoutList(c.Breakouts), c.BaselinePosts)
	writeBlueprint(b, c.Blueprint)
	writeSkipped(b, c.Skipped)
}

func breakoutList(breakouts []Breakout) string {
	if len(breakouts) == 0 {
		return "none"
	}
	items := make([]string, len(breakouts))
	for i, br := range breakouts {
		items[i] = fmt.Sprintf("%s at %.2fx", br.ID, br.Multiplier)
	}
	return fmt.Sprintf("%d (%s)", len(breakouts), strings.Join(items, ", "))
}

func writeBlueprint(b *strings.Builder, bp *blueprint.Blueprint) {
	if bp == nil {
		return
	}
	fmt.Fprintf(b, "**Edit blueprint**: %d breakouts against %d baseline posts.\n\n", bp.Breakouts, bp.Baseline)
	for _, s := range bp.Sentences {
		fmt.Fprintf(b, "- %s\n", s)
	}
	if len(bp.Sentences) > 0 {
		b.WriteString("\n")
	}
	writeMetricTable(b, bp.Metrics)
	for _, n := range bp.Notes {
		fmt.Fprintf(b, "- %s\n", n)
	}
	if len(bp.Notes) > 0 {
		b.WriteString("\n")
	}
}

func writeMetricTable(b *strings.Builder, metrics []blueprint.Metric) {
	b.WriteString("| Metric | Baseline posts | Breakouts | Change |\n|---|---|---|---|\n")
	for _, m := range metrics {
		fmt.Fprintf(b, "| %s | %s | %s | %s |\n", m.Label, blueprint.Display(m.Unit, m.Before), blueprint.Display(m.Unit, m.After), changeCell(m))
	}
	b.WriteString("\n")
}

func changeCell(m blueprint.Metric) string {
	if m.Change == nil {
		return "from zero"
	}
	return blueprint.SignedPercent(*m.Change)
}

func writeSkipped(b *strings.Builder, skipped []Skip) {
	if len(skipped) == 0 {
		return
	}
	b.WriteString("Not computed:\n\n")
	for _, s := range skipped {
		if s.Post != "" {
			fmt.Fprintf(b, "- Post %s: %s, %s.\n", s.Post, s.Reason, s.Detail)
		} else {
			fmt.Fprintf(b, "- %s, %s.\n", s.Reason, s.Detail)
		}
	}
	b.WriteString("\n")
}

func writeFailures(b *strings.Builder, failures []Failure) {
	b.WriteString("## Failures\n\n")
	if len(failures) == 0 {
		b.WriteString("None.\n")
		return
	}
	for _, f := range failures {
		fmt.Fprintf(b, "- %s, post %s: %s\n", f.Creator, f.Post, failureText(f))
	}
}

func failureText(f Failure) string {
	switch {
	case f.Stage == StageReadDNA:
		return "the edit_dna.json could not be used: " + f.Detail
	case f.ExitCode == -1:
		return "reelctl did not start: " + f.Detail
	default:
		return fmt.Sprintf("reelctl exited with code %d: %s", f.ExitCode, f.Detail)
	}
}
