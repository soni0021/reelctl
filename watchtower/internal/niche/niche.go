// Package niche holds the posts and follower snapshots the watchtower reads,
// and FileSource, which reads them from recorded JSON files.
package niche

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

const (
	dateLayout = "2006-01-02"
	fileName   = "niche.json"
)

// Snapshot is a creator's follower count and total views on one day.
type Snapshot struct {
	Date       time.Time
	Followers  int64
	TotalViews int64
}

// Post is one Reel. Media is the path of the video file, empty when there is none.
type Post struct {
	ID       string
	URL      string
	PostedAt time.Time
	Views    int64
	Media    string
}

// Creator is one account in the niche.
type Creator struct {
	Handle    string
	Snapshots []Snapshot
	Posts     []Post
}

// Niche is everything one run reads.
type Niche struct {
	Name     string
	Creators []Creator
}

// Source supplies a niche. FileSource is the only implementation.
type Source interface {
	Load() (Niche, error)
}

// FileSource reads niche.json from Dir. Media paths in it are relative to Dir.
type FileSource struct {
	Dir string
}

type rawNiche struct {
	Niche    *string      `json:"niche"`
	Creators []rawCreator `json:"creators"`
}

type rawCreator struct {
	Handle    *string       `json:"handle"`
	Snapshots []rawSnapshot `json:"snapshots"`
	Posts     []rawPost     `json:"posts"`
}

type rawSnapshot struct {
	Date       *string `json:"date"`
	Followers  *int64  `json:"followers"`
	TotalViews *int64  `json:"total_views"`
}

type rawPost struct {
	ID        *string `json:"id"`
	URL       string  `json:"url"`
	PostedAt  *string `json:"posted_at"`
	Views     *int64  `json:"views"`
	MediaPath string  `json:"media_path"`
}

// Load reads and checks niche.json.
func (f FileSource) Load() (Niche, error) {
	data, err := os.ReadFile(filepath.Join(f.Dir, fileName))
	if err != nil {
		return Niche{}, err
	}
	var raw rawNiche
	if err := json.Unmarshal(data, &raw); err != nil {
		return Niche{}, fmt.Errorf("%s: %w", fileName, err)
	}
	return f.build(raw)
}

func (f FileSource) build(raw rawNiche) (Niche, error) {
	if raw.Niche == nil {
		return Niche{}, fmt.Errorf("%s: missing field niche", fileName)
	}
	n := Niche{Name: *raw.Niche}
	for i, rc := range raw.Creators {
		c, err := f.buildCreator(i+1, rc)
		if err != nil {
			return Niche{}, err
		}
		n.Creators = append(n.Creators, c)
	}
	return n, nil
}

func (f FileSource) buildCreator(position int, rc rawCreator) (Creator, error) {
	if rc.Handle == nil {
		return Creator{}, fmt.Errorf("creator #%d: missing field handle", position)
	}
	label := fmt.Sprintf("creator %q", *rc.Handle)
	c := Creator{Handle: *rc.Handle}
	for i, rs := range rc.Snapshots {
		s, err := buildSnapshot(i+1, rs)
		if err != nil {
			return Creator{}, fmt.Errorf("%s: %w", label, err)
		}
		c.Snapshots = append(c.Snapshots, s)
	}
	for i, rp := range rc.Posts {
		p, err := f.buildPost(i+1, rp)
		if err != nil {
			return Creator{}, fmt.Errorf("%s: %w", label, err)
		}
		c.Posts = append(c.Posts, p)
	}
	return c, nil
}

func buildSnapshot(position int, rs rawSnapshot) (Snapshot, error) {
	label := fmt.Sprintf("snapshot %d", position)
	if rs.Date == nil {
		return Snapshot{}, fmt.Errorf("%s: missing field date", label)
	}
	if rs.Followers == nil {
		return Snapshot{}, fmt.Errorf("%s: missing field followers", label)
	}
	if rs.TotalViews == nil {
		return Snapshot{}, fmt.Errorf("%s: missing field total_views", label)
	}
	date, err := time.Parse(dateLayout, *rs.Date)
	if err != nil {
		return Snapshot{}, fmt.Errorf("%s: field date: %w", label, err)
	}
	return Snapshot{Date: date, Followers: *rs.Followers, TotalViews: *rs.TotalViews}, nil
}

func (f FileSource) buildPost(position int, rp rawPost) (Post, error) {
	if rp.ID == nil {
		return Post{}, fmt.Errorf("post %d: missing field id", position)
	}
	label := fmt.Sprintf("post %q", *rp.ID)
	if rp.PostedAt == nil {
		return Post{}, fmt.Errorf("%s: missing field posted_at", label)
	}
	if rp.Views == nil {
		return Post{}, fmt.Errorf("%s: missing field views", label)
	}
	at, err := time.Parse(time.RFC3339, *rp.PostedAt)
	if err != nil {
		return Post{}, fmt.Errorf("%s: field posted_at: %w", label, err)
	}
	return Post{ID: *rp.ID, URL: rp.URL, PostedAt: at, Views: *rp.Views, Media: f.mediaFile(rp.MediaPath)}, nil
}

func (f FileSource) mediaFile(mediaPath string) string {
	if mediaPath == "" {
		return ""
	}
	return filepath.Join(f.Dir, mediaPath)
}
