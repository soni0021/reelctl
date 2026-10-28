package blueprint

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func loadErr(t *testing.T, body string) error {
	t.Helper()
	path := filepath.Join(t.TempDir(), "dna.json")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := LoadDNA(path)
	return err
}

func TestLoadDNAAcceptsSchemaVersionsOneToFour(t *testing.T) {
	for _, v := range []string{"1", "2", "3", "4"} {
		body := strings.Replace(`{`+minimal+`}`, `"schema_version": 2`, `"schema_version": `+v, 1)
		if err := loadErr(t, body); err != nil {
			t.Errorf("version %s: %v", v, err)
		}
	}
}

func TestLoadDNARejectsAnUnknownOrMissingSchemaVersion(t *testing.T) {
	cases := map[string]string{
		"zero":    strings.Replace(`{`+minimal+`}`, `"schema_version": 2`, `"schema_version": 0`, 1),
		"five":    strings.Replace(`{`+minimal+`}`, `"schema_version": 2`, `"schema_version": 5`, 1),
		"null":    strings.Replace(`{`+minimal+`}`, `"schema_version": 2`, `"schema_version": null`, 1),
		"missing": strings.Replace(`{`+minimal+`}`, `"schema_version": 2, `, ``, 1),
	}
	for name, body := range cases {
		err := loadErr(t, body)
		if err == nil || !strings.Contains(err.Error(), "schema_version") {
			t.Errorf("%s: got %v", name, err)
		}
	}
}

func TestLoadDNARejectsAMissingOrNullNumericField(t *testing.T) {
	fields := map[string]string{
		"duration_ms":      `"duration_ms": 1000`,
		"cuts":             `"cuts": []`,
		"silence_gaps":     `"silence_gaps": []`,
		"words_per_minute": `"words_per_minute": 0`,
		"integrated_lufs":  `"integrated_lufs": -70`,
	}
	for name, field := range fields {
		missing := strings.Replace(`{`+minimal+`}`, field+", ", "", 1)
		missing = strings.Replace(missing, ", "+field, "", 1)
		if err := loadErr(t, missing); err == nil || !strings.Contains(err.Error(), name) {
			t.Errorf("%s absent: got %v", name, err)
		}
		null := strings.Replace(`{`+minimal+`}`, field, `"`+name+`": null`, 1)
		if err := loadErr(t, null); err == nil || !strings.Contains(err.Error(), name) {
			t.Errorf("%s null: got %v", name, err)
		}
	}
}

func TestLoadDNARejectsANullCutTimeOrGapDuration(t *testing.T) {
	cut := strings.Replace(`{`+minimal+`}`, `"cuts": []`, `"cuts": [{"time_ms": null}]`, 1)
	if err := loadErr(t, cut); err == nil || !strings.Contains(err.Error(), "time_ms") {
		t.Errorf("cut: got %v", err)
	}
	gap := strings.Replace(`{`+minimal+`}`, `"silence_gaps": []`, `"silence_gaps": [{"start_ms": 1}]`, 1)
	if err := loadErr(t, gap); err == nil || !strings.Contains(err.Error(), "duration_ms") {
		t.Errorf("gap: got %v", err)
	}
}

func TestLoadDNARejectsANonPositiveDuration(t *testing.T) {
	for _, d := range []string{"0", "-5"} {
		body := strings.Replace(`{`+minimal+`}`, `"duration_ms": 1000`, `"duration_ms": `+d, 1)
		if err := loadErr(t, body); err == nil || !strings.Contains(err.Error(), "duration_ms") {
			t.Errorf("duration %s: got %v", d, err)
		}
	}
}

func TestLoadDNAKeepsTheASRBackend(t *testing.T) {
	got := loadFrom(t, `{`+minimal+`, "asr_backend": "silence_bursts"}`)
	if got.ASRBackend != "silence_bursts" {
		t.Fatalf("got %q", got.ASRBackend)
	}
	r, err := Measure(got)
	if err != nil || r.ASRBackend != "silence_bursts" {
		t.Fatalf("got %+v %v", r, err)
	}
}

func reelWith(backend string, rate float64) Reel {
	return Reel{ASRBackend: backend, WordsPerMinute: rate, MeanShotMs: 2000, MedianShotMs: 2000, LUFS: -14}
}

func groups(backend string, before, after float64) ([]Reel, []Reel) {
	return []Reel{reelWith(backend, before), reelWith(backend, before)}, []Reel{reelWith(backend, after), reelWith(backend, after)}
}

func TestWhisperRatesAreLabelledWordsPerMinute(t *testing.T) {
	bp := Compare(groups(BackendWhisper, 100, 150))
	m := find(t, bp, "words_per_minute")
	if m.Label != "Words per minute" {
		t.Fatalf("label %q", m.Label)
	}
	if len(bp.Sentences) != 1 || !strings.HasPrefix(bp.Sentences[0], "Words per minute rose") {
		t.Fatalf("sentences %q", bp.Sentences)
	}
}

func TestBurstRatesAreLabelledVoicedBurstsPerMinuteNotWords(t *testing.T) {
	bp := Compare(groups(BackendSilenceBursts, 100, 150))
	m := find(t, bp, "voiced_bursts_per_minute")
	if m.Label != "Voiced bursts per minute" || m.Before != 100 || m.After != 150 {
		t.Fatalf("got %+v", m)
	}
	for _, metric := range bp.Metrics {
		if metric.Key == "words_per_minute" {
			t.Fatal("a burst rate must not be called words per minute")
		}
	}
	joined := strings.Join(bp.Sentences, "\n")
	if !strings.Contains(joined, "Voiced bursts per minute rose from 100.0 to 150.0 (+50%)") || strings.Contains(joined, "Words per minute") {
		t.Fatalf("sentences %q", bp.Sentences)
	}
}

func TestMixedBackendsOmitTheRateAndSayWhy(t *testing.T) {
	baseline, _ := groups(BackendSilenceBursts, 100, 0)
	_, breakouts := groups(BackendWhisper, 0, 150)
	bp := Compare(baseline, breakouts)
	for _, m := range bp.Metrics {
		if m.Key == "words_per_minute" || m.Key == "voiced_bursts_per_minute" {
			t.Fatalf("metric %q compared across backends", m.Key)
		}
	}
	want := "asr_backend cannot be compared across the files (2 silence_bursts, 2 whisper_cpp), so the speech rate is not compared: a voiced-burst rate is not a speaking rate."
	if !containsNote(bp.Notes, want) {
		t.Fatalf("notes %q, want %q", bp.Notes, want)
	}
}

func TestMixingInsideOneGroupIsAlsoOmitted(t *testing.T) {
	baseline := []Reel{reelWith(BackendWhisper, 100), reelWith(BackendSilenceBursts, 100)}
	_, breakouts := groups(BackendWhisper, 0, 150)
	bp := Compare(baseline, breakouts)
	for _, m := range bp.Metrics {
		if strings.HasSuffix(m.Key, "_per_minute") && (strings.HasPrefix(m.Key, "words") || strings.HasPrefix(m.Key, "voiced")) {
			t.Fatalf("metric %q compared though the baseline mixes backends", m.Key)
		}
	}
	if len(bp.Notes) == 0 {
		t.Fatal("no note")
	}
}

func TestAMissingBackendOmitsTheRate(t *testing.T) {
	baseline, breakouts := groups("", 100, 150)
	bp := Compare(baseline, breakouts)
	for _, m := range bp.Metrics {
		if m.Key == "words_per_minute" || m.Key == "voiced_bursts_per_minute" {
			t.Fatalf("metric %q compared with no asr_backend", m.Key)
		}
	}
	want := "asr_backend cannot be compared across the files (4 missing), so the speech rate is not compared: a voiced-burst rate is not a speaking rate."
	if !containsNote(bp.Notes, want) {
		t.Fatalf("notes %q", bp.Notes)
	}
}

func TestNotesKeepVisualBeforeSpeech(t *testing.T) {
	baseline, breakouts := groups("", 1, 2)
	bp := Compare(baseline, breakouts)
	if len(bp.Notes) != 2 || !strings.HasPrefix(bp.Notes[0], "visual_events") || !strings.HasPrefix(bp.Notes[1], "asr_backend") {
		t.Fatalf("notes %q", bp.Notes)
	}
}

func TestTheMetricOrderHoldsWhenTheRateIsKept(t *testing.T) {
	bp := Compare(groups(BackendSilenceBursts, 1, 2))
	var keys []string
	for _, m := range bp.Metrics {
		keys = append(keys, m.Key)
	}
	want := []string{"mean_shot_length_ms", "median_shot_length_ms", "cuts_per_minute", "pauses_per_minute", "silent_share", "voiced_bursts_per_minute", "integrated_lufs"}
	if !reflect.DeepEqual(keys, want) {
		t.Fatalf("got %v", keys)
	}
}

func containsNote(notes []string, want string) bool {
	for _, n := range notes {
		if n == want {
			return true
		}
	}
	return false
}
