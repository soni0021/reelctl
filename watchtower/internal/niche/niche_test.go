package niche

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeNiche(t *testing.T, body string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "niche.json"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

const valid = `{"niche": "neet-prep", "extra": 1, "creators": [
  {"handle": "a", "ignored": true,
   "snapshots": [{"date": "2026-10-21", "followers": 100000, "total_views": 5000000}],
   "posts": [
     {"id": "p1", "url": "https://example.test/p1", "posted_at": "2026-10-20T10:00:00Z", "views": 250000, "media_path": "media/p1.mp4"},
     {"id": "p2", "posted_at": "2026-10-19T10:00:00Z", "views": 1000}
   ]}
]}`

func TestLoadReadsAValidDirectory(t *testing.T) {
	dir := writeNiche(t, valid)
	n, err := FileSource{Dir: dir}.Load()
	if err != nil {
		t.Fatal(err)
	}
	if n.Name != "neet-prep" || len(n.Creators) != 1 {
		t.Fatalf("got %+v", n)
	}
	c := n.Creators[0]
	if c.Handle != "a" || len(c.Snapshots) != 1 || len(c.Posts) != 2 {
		t.Fatalf("got %+v", c)
	}
	if c.Snapshots[0].Followers != 100000 || c.Snapshots[0].TotalViews != 5000000 {
		t.Fatalf("snapshot: %+v", c.Snapshots[0])
	}
	if c.Posts[0].Views != 250000 || c.Posts[0].URL != "https://example.test/p1" {
		t.Fatalf("post: %+v", c.Posts[0])
	}
}

func TestLoadResolvesMediaAgainstTheDirectory(t *testing.T) {
	dir := writeNiche(t, valid)
	n, err := FileSource{Dir: dir}.Load()
	if err != nil {
		t.Fatal(err)
	}
	posts := n.Creators[0].Posts
	if want := filepath.Join(dir, "media", "p1.mp4"); posts[0].Media != want {
		t.Errorf("media = %q, want %q", posts[0].Media, want)
	}
	if posts[1].Media != "" {
		t.Errorf("a post without media_path got media %q", posts[1].Media)
	}
}

func TestLoadNamesTheCreatorAndTheMissingField(t *testing.T) {
	cases := []struct {
		name, body string
		wants      []string
	}{
		{"niche name", `{"creators": []}`, []string{"niche"}},
		{"creator handle", `{"niche": "n", "creators": [{"handle": "a"}, {"snapshots": []}]}`, []string{"creator #2", "handle"}},
		{"snapshot date", `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"followers": 1, "total_views": 1}]}]}`, []string{`"a"`, "snapshot 1", "date"}},
		{"snapshot followers", `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"date": "2026-10-21", "total_views": 1}]}]}`, []string{`"a"`, "followers"}},
		{"snapshot total views", `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"date": "2026-10-21", "followers": 1}]}]}`, []string{`"a"`, "total_views"}},
		{"post id", `{"niche": "n", "creators": [{"handle": "a", "posts": [{"posted_at": "2026-10-20T10:00:00Z", "views": 1}]}]}`, []string{`"a"`, "post 1", "id"}},
		{"post time", `{"niche": "n", "creators": [{"handle": "a", "posts": [{"id": "p1", "views": 1}]}]}`, []string{`"a"`, `"p1"`, "posted_at"}},
		{"post views", `{"niche": "n", "creators": [{"handle": "a", "posts": [{"id": "p1", "posted_at": "2026-10-20T10:00:00Z"}]}]}`, []string{`"a"`, `"p1"`, "views"}},
	}
	for _, c := range cases {
		_, err := FileSource{Dir: writeNiche(t, c.body)}.Load()
		if err == nil {
			t.Errorf("%s: no error", c.name)
			continue
		}
		for _, want := range c.wants {
			if !strings.Contains(err.Error(), want) {
				t.Errorf("%s: error %q does not mention %q", c.name, err, want)
			}
		}
	}
}

func TestLoadRejectsABadDateNamingTheCreator(t *testing.T) {
	body := `{"niche": "n", "creators": [{"handle": "a", "snapshots": [{"date": "21/10/2026", "followers": 1, "total_views": 1}]}]}`
	_, err := FileSource{Dir: writeNiche(t, body)}.Load()
	if err == nil || !strings.Contains(err.Error(), `"a"`) || !strings.Contains(err.Error(), "date") {
		t.Fatalf("got %v", err)
	}
}

func TestLoadRejectsABadPostTimeNamingThePost(t *testing.T) {
	body := `{"niche": "n", "creators": [{"handle": "a", "posts": [{"id": "p1", "posted_at": "yesterday", "views": 1}]}]}`
	_, err := FileSource{Dir: writeNiche(t, body)}.Load()
	if err == nil || !strings.Contains(err.Error(), `"p1"`) || !strings.Contains(err.Error(), "posted_at") {
		t.Fatalf("got %v", err)
	}
}

func TestLoadFailsOnAMissingDirectory(t *testing.T) {
	if _, err := (FileSource{Dir: filepath.Join(t.TempDir(), "absent")}).Load(); err == nil {
		t.Fatal("expected an error")
	}
}

func TestLoadFailsOnInvalidJSON(t *testing.T) {
	if _, err := (FileSource{Dir: writeNiche(t, `{"niche":`)}).Load(); err == nil {
		t.Fatal("expected an error")
	}
}
