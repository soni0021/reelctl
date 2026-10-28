package niche

import (
	"fmt"
	"regexp"
	"strings"
)

var safeID = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)

// CheckIDs checks every post id in n. An id must not be empty, must use only
// letters, digits, '.', '_' and '-', and must not contain ".." or be only
// dots, because it names the file dna/<id>.edit_dna.json. No two posts in the
// whole niche may share an id, ignoring case, because a case-insensitive file
// system would give them one file. The error names the creator and the id.
func CheckIDs(n Niche) error {
	seen := make(map[string]string)
	for _, c := range n.Creators {
		for i, p := range c.Posts {
			if err := checkID(p.ID); err != nil {
				return fmt.Errorf("creator %q: %s: field id: %w", c.Handle, postLabel(i+1, p.ID), err)
			}
			key := strings.ToLower(p.ID)
			if owner, dup := seen[key]; dup {
				return fmt.Errorf("creator %q: post %q: duplicate id (ignoring case), already used by creator %q", c.Handle, p.ID, owner)
			}
			seen[key] = c.Handle
		}
	}
	return nil
}

func postLabel(position int, id string) string {
	if id == "" {
		return fmt.Sprintf("post %d", position)
	}
	return fmt.Sprintf("post %q", id)
}

func checkID(id string) error {
	switch {
	case id == "":
		return fmt.Errorf("must not be empty")
	case !safeID.MatchString(id):
		return fmt.Errorf("may only use letters, digits, '.', '_' and '-'")
	case strings.Contains(id, ".."), strings.Trim(id, ".") == "":
		return fmt.Errorf("must not contain \"..\" or be only dots")
	}
	return nil
}
