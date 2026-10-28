package niche

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func nicheWithPosts(posts ...string) string {
	return `{"niche": "n", "creators": [{"handle": "a", "posts": [` + strings.Join(posts, ",") + `]}]}`
}

func postWithMedia(media string) string {
	b, _ := json.Marshal(media)
	return `{"id": "p1", "posted_at": "2026-10-20T10:00:00Z", "views": 1, "media_path": ` + string(b) + `}`
}

func postWithID(id string) string {
	b, _ := json.Marshal(id)
	return `{"id": ` + string(b) + `, "posted_at": "2026-10-20T10:00:00Z", "views": 1}`
}

func TestLoadRejectsAMediaPathThatLeavesTheDataDirectory(t *testing.T) {
	cases := map[string]string{
		"parent":       "../x",
		"absolute":     "/abs/x.mp4",
		"up and out":   "a/../../b",
		"leading dash": "-rf.mp4",
		"just parent":  "..",
		"deep parent":  "media/../../../etc/passwd",
		"just dash":    "-",
	}
	for name, media := range cases {
		_, err := FileSource{Dir: writeNiche(t, nicheWithPosts(postWithMedia(media)))}.Load()
		if err == nil {
			t.Errorf("%s (%q): no error", name, media)
			continue
		}
		for _, want := range []string{`creator "a"`, `post "p1"`, "media_path"} {
			if !strings.Contains(err.Error(), want) {
				t.Errorf("%s: error %q does not mention %q", name, err, want)
			}
		}
	}
}

// The symlink check would also catch a ".." that lands outside, but only
// after resolving paths on disk; the plain check says so by name.
func TestLoadNamesTheDotDotWhenAMediaPathClimbsOut(t *testing.T) {
	for _, media := range []string{"../x", "a/../../b", ".."} {
		_, err := FileSource{Dir: writeNiche(t, nicheWithPosts(postWithMedia(media)))}.Load()
		if err == nil || !strings.Contains(err.Error(), "climbs out of the data directory") {
			t.Errorf("%q: got %v", media, err)
		}
	}
}

func TestLoadAcceptsAMediaPathThatStaysInside(t *testing.T) {
	dir := writeNiche(t, nicheWithPosts(postWithMedia("media/../clips/./p1.mp4")))
	n, err := FileSource{Dir: dir}.Load()
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(dir, "clips", "p1.mp4"); n.Creators[0].Posts[0].Media != want {
		t.Fatalf("media = %q, want %q", n.Creators[0].Posts[0].Media, want)
	}
}

func TestLoadPassesAnAbsoluteMediaPathForARelativeDataDirectory(t *testing.T) {
	dir := writeNiche(t, nicheWithPosts(postWithMedia("media/p1.mp4")))
	t.Chdir(filepath.Dir(dir))
	n, err := FileSource{Dir: filepath.Base(dir)}.Load()
	if err != nil {
		t.Fatal(err)
	}
	if got := n.Creators[0].Posts[0].Media; !filepath.IsAbs(got) {
		t.Fatalf("media %q is not absolute", got)
	}
}

func TestLoadRejectsAFileSymlinkThatPointsOutside(t *testing.T) {
	dir := writeNiche(t, nicheWithPosts(postWithMedia("media/link.mp4")))
	outside := filepath.Join(t.TempDir(), "secret.mp4")
	if err := os.WriteFile(outside, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(dir, "media"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dir, "media", "link.mp4")); err != nil {
		t.Fatal(err)
	}
	_, err := FileSource{Dir: dir}.Load()
	if err == nil || !strings.Contains(err.Error(), "media_path") || !strings.Contains(err.Error(), `post "p1"`) {
		t.Fatalf("got %v", err)
	}
}

func TestLoadRejectsADirectorySymlinkThatPointsOutsideEvenForAMissingFile(t *testing.T) {
	dir := writeNiche(t, nicheWithPosts(postWithMedia("media/p1.mp4")))
	if err := os.Symlink(t.TempDir(), filepath.Join(dir, "media")); err != nil {
		t.Fatal(err)
	}
	_, err := FileSource{Dir: dir}.Load()
	if err == nil || !strings.Contains(err.Error(), "media_path") {
		t.Fatalf("got %v", err)
	}
}

func TestLoadAcceptsASymlinkThatStaysInside(t *testing.T) {
	dir := writeNiche(t, nicheWithPosts(postWithMedia("media/link.mp4")))
	if err := os.Mkdir(filepath.Join(dir, "media"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "real.mp4"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(dir, "real.mp4"), filepath.Join(dir, "media", "link.mp4")); err != nil {
		t.Fatal(err)
	}
	if _, err := (FileSource{Dir: dir}).Load(); err != nil {
		t.Fatal(err)
	}
}

func TestLoadRejectsABadPostID(t *testing.T) {
	cases := map[string]string{
		"empty":     "",
		"slash":     "a/b",
		"backslash": `a\b`,
		"dotdot":    "..",
		"embedded":  "a..b",
		"space":     "a b",
		"dot":       ".",
		"unicode":   "é",
	}
	for name, id := range cases {
		_, err := FileSource{Dir: writeNiche(t, nicheWithPosts(postWithID(id)))}.Load()
		if err == nil {
			t.Errorf("%s (%q): no error", name, id)
			continue
		}
		if !strings.Contains(err.Error(), `creator "a"`) || !strings.Contains(err.Error(), "id") {
			t.Errorf("%s: error %q does not name the creator and the field", name, err)
		}
	}
}

func TestLoadRejectsADuplicatePostIDAcrossCreators(t *testing.T) {
	body := `{"niche": "n", "creators": [
	  {"handle": "a", "posts": [` + postWithID("p1") + `]},
	  {"handle": "b", "posts": [` + postWithID("p2") + `,` + postWithID("p1") + `]}]}`
	_, err := FileSource{Dir: writeNiche(t, body)}.Load()
	if err == nil {
		t.Fatal("no error")
	}
	for _, want := range []string{`creator "b"`, `"p1"`, "duplicate", `creator "a"`} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("error %q does not mention %q", err, want)
		}
	}
}

func TestLoadRejectsPostIDsThatDifferOnlyInCase(t *testing.T) {
	_, err := FileSource{Dir: writeNiche(t, nicheWithPosts(postWithID("P1"), postWithID("p1")))}.Load()
	if err == nil || !strings.Contains(err.Error(), "duplicate") {
		t.Fatalf("got %v", err)
	}
}

func TestLoadRejectsNegativeCounts(t *testing.T) {
	cases := []struct{ name, body, field string }{
		{"followers", `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"date": "2026-10-21", "followers": -1, "total_views": 1}]}]}`, "followers"},
		{"total views", `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"date": "2026-10-21", "followers": 1, "total_views": -5}]}]}`, "total_views"},
		{"views", nicheWithPosts(`{"id": "p1", "posted_at": "2026-10-20T10:00:00Z", "views": -3}`), "views"},
	}
	for _, c := range cases {
		_, err := FileSource{Dir: writeNiche(t, c.body)}.Load()
		if err == nil || !strings.Contains(err.Error(), c.field) || !strings.Contains(err.Error(), `"a"`) || !strings.Contains(err.Error(), "negative") {
			t.Errorf("%s: got %v", c.name, err)
		}
	}
}

func TestCheckIDsAppliesToAnyNiche(t *testing.T) {
	n := Niche{Creators: []Creator{{Handle: "a", Posts: []Post{{ID: "x"}}}, {Handle: "b", Posts: []Post{{ID: "X"}}}}}
	if err := CheckIDs(n); err == nil {
		t.Fatal("expected a duplicate error")
	}
}
