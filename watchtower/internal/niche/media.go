package niche

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// mediaFile turns a media_path into the absolute path of a file inside the
// data directory. It rejects a path that is absolute, starts with '-',
// climbs out with a ".." element, or whose nearest existing part resolves
// (through symlinks) outside the directory. An empty path means no media.
// The file need not exist: a missing file is a decompile failure later.
func (f FileSource) mediaFile(mediaPath string) (string, error) {
	if mediaPath == "" {
		return "", nil
	}
	if strings.ContainsRune(mediaPath, 0) {
		return "", errors.New("contains a NUL byte")
	}
	if strings.HasPrefix(mediaPath, "-") {
		return "", fmt.Errorf("%q starts with '-'", mediaPath)
	}
	if filepath.IsAbs(mediaPath) || strings.HasPrefix(mediaPath, "/") || strings.HasPrefix(mediaPath, `\`) || filepath.VolumeName(mediaPath) != "" {
		return "", fmt.Errorf("%q is absolute; it must be relative to the data directory", mediaPath)
	}
	clean := filepath.Clean(mediaPath)
	for _, element := range strings.Split(filepath.ToSlash(clean), "/") {
		if element == ".." {
			return "", fmt.Errorf("%q climbs out of the data directory", mediaPath)
		}
	}
	root, err := filepath.Abs(f.Dir)
	if err != nil {
		return "", err
	}
	full := filepath.Join(root, clean)
	if err := insideAfterSymlinks(root, full); err != nil {
		return "", fmt.Errorf("%q: %w", mediaPath, err)
	}
	return full, nil
}

// insideAfterSymlinks checks that full, or its nearest existing ancestor,
// resolves to a place inside root.
func insideAfterSymlinks(root, full string) error {
	realRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return err
	}
	existing := full
	for {
		if _, err := os.Lstat(existing); err == nil {
			break
		}
		parent := filepath.Dir(existing)
		if parent == existing {
			return errors.New("no part of the path exists")
		}
		existing = parent
	}
	real, err := filepath.EvalSymlinks(existing)
	if err != nil {
		return fmt.Errorf("cannot be resolved: %w", err)
	}
	rel, err := filepath.Rel(realRoot, real)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return errors.New("resolves outside the data directory through a symlink")
	}
	return nil
}
