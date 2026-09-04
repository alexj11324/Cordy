package cli

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestReleaseDistributionContract(t *testing.T) {
	if GitHubReleaseRepository != "alexj11324/Cordy" {
		t.Fatalf("release repository = %q", GitHubReleaseRepository)
	}
	if GitHubReleaseWebURL != "https://github.com/alexj11324/Cordy/releases" {
		t.Fatalf("release web URL = %q", GitHubReleaseWebURL)
	}
	if HomebrewPackage != "alexj11324/tap/patchbay" {
		t.Fatalf("Homebrew package = %q", HomebrewPackage)
	}
}

func withBrewCommandStub(t *testing.T, stub func(args ...string) (string, error)) {
	t.Helper()
	previous := brewCommand
	brewCommand = stub
	t.Cleanup(func() { brewCommand = previous })
}

func TestUpdateViaBrewMigratesLegacyFormulaToCask(t *testing.T) {
	var calls [][]string
	withBrewCommandStub(t, func(args ...string) (string, error) {
		calls = append(calls, append([]string(nil), args...))
		switch strings.Join(args, " ") {
		case "list --formula alexj11324/Cordy/patchbay":
			return "patchbay 0.2.8", nil
		case "list --cask " + HomebrewPackage:
			return "", errors.New("not installed")
		default:
			return strings.Join(args, " "), nil
		}
	})

	if _, err := UpdateViaBrew(); err != nil {
		t.Fatalf("UpdateViaBrew() error = %v", err)
	}
	want := [][]string{
		{"list", "--formula", "alexj11324/Cordy/patchbay"},
		{"list", "--cask", HomebrewPackage},
		{"unlink", "alexj11324/Cordy/patchbay"},
		{"install", "--cask", HomebrewPackage},
		{"uninstall", "--formula", "--ignore-dependencies", "alexj11324/Cordy/patchbay"},
	}
	if !reflect.DeepEqual(calls, want) {
		t.Fatalf("brew calls = %#v, want %#v", calls, want)
	}
}

func TestUpdateViaBrewRelinksLegacyFormulaWhenCaskInstallFails(t *testing.T) {
	var calls [][]string
	installErr := errors.New("cask install failed")
	withBrewCommandStub(t, func(args ...string) (string, error) {
		calls = append(calls, append([]string(nil), args...))
		switch strings.Join(args, " ") {
		case "list --formula alexj11324/Cordy/patchbay":
			return "patchbay 0.2.8", nil
		case "list --cask " + HomebrewPackage:
			return "", errors.New("not installed")
		case "install --cask " + HomebrewPackage:
			return "install failed", installErr
		default:
			return strings.Join(args, " "), nil
		}
	})

	if _, err := UpdateViaBrew(); !errors.Is(err, installErr) {
		t.Fatalf("UpdateViaBrew() error = %v, want %v", err, installErr)
	}
	if got := calls[len(calls)-1]; !reflect.DeepEqual(got, []string{"link", "alexj11324/Cordy/patchbay"}) {
		t.Fatalf("final brew call = %#v, want legacy relink", got)
	}
}

func TestUpdateViaBrewUpgradesCurrentCask(t *testing.T) {
	var calls [][]string
	withBrewCommandStub(t, func(args ...string) (string, error) {
		calls = append(calls, append([]string(nil), args...))
		if len(args) > 1 && args[0] == "list" && args[1] == "--formula" {
			return "", errors.New("not installed")
		}
		return "upgraded", nil
	})

	if _, err := UpdateViaBrew(); err != nil {
		t.Fatalf("UpdateViaBrew() error = %v", err)
	}
	wantLast := []string{"upgrade", HomebrewPackage}
	if got := calls[len(calls)-1]; !reflect.DeepEqual(got, wantLast) {
		t.Fatalf("final brew call = %#v, want %#v", got, wantLast)
	}
}

func TestReleaseAssetCandidates(t *testing.T) {
	tests := []struct {
		name          string
		targetVersion string
		goos          string
		goarch        string
		wantAssets    []string
	}{
		{
			name:          "darwin prefers versioned then legacy candidate",
			targetVersion: "v1.2.3",
			goos:          "darwin",
			goarch:        "arm64",
			wantAssets: []string{
				"patchbay-cli-1.2.3-darwin-arm64.tar.gz",
				"patchbay_darwin_arm64.tar.gz",
			},
		},
		{
			name:          "linux normalizes missing v in versioned candidate",
			targetVersion: "1.2.3",
			goos:          "linux",
			goarch:        "amd64",
			wantAssets: []string{
				"patchbay-cli-1.2.3-linux-amd64.tar.gz",
				"patchbay_linux_amd64.tar.gz",
			},
		},
		{
			name:          "windows uses zip assets",
			targetVersion: "1.2.3",
			goos:          "windows",
			goarch:        "amd64",
			wantAssets: []string{
				"patchbay-cli-1.2.3-windows-amd64.zip",
				"patchbay_windows_amd64.zip",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := releaseAssetCandidates(tt.targetVersion, tt.goos, tt.goarch)
			if len(got) != len(tt.wantAssets) {
				t.Fatalf("candidate count mismatch: got %d, want %d", len(got), len(tt.wantAssets))
			}
			for i := range got {
				if got[i] != tt.wantAssets[i] {
					t.Fatalf("candidate[%d] mismatch: got %q, want %q", i, got[i], tt.wantAssets[i])
				}
			}
		})
	}
}

func TestFindReleaseAsset(t *testing.T) {
	t.Run("prefers versioned asset when both names exist", func(t *testing.T) {
		assets := []GitHubReleaseAsset{
			{Name: "patchbay_darwin_amd64.tar.gz", BrowserDownloadURL: "old"},
			{Name: "patchbay-cli-1.2.3-darwin-amd64.tar.gz", BrowserDownloadURL: "new"},
		}

		got, err := findReleaseAsset(assets, "v1.2.3", "darwin", "amd64")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got.Name != "patchbay-cli-1.2.3-darwin-amd64.tar.gz" {
			t.Fatalf("asset mismatch: got %q", got.Name)
		}
	})

	t.Run("falls back to legacy asset when versioned is absent", func(t *testing.T) {
		assets := []GitHubReleaseAsset{
			{Name: "patchbay_linux_amd64.tar.gz", BrowserDownloadURL: "old"},
		}

		got, err := findReleaseAsset(assets, "1.2.3", "linux", "amd64")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got.Name != "patchbay_linux_amd64.tar.gz" {
			t.Fatalf("asset mismatch: got %q", got.Name)
		}
	})

	t.Run("returns error when no candidate matches", func(t *testing.T) {
		_, err := findReleaseAsset([]GitHubReleaseAsset{{Name: "checksums.txt"}}, "1.2.3", "linux", "amd64")
		if err == nil {
			t.Fatal("expected error, got nil")
		}
	})
}

func TestIsReleaseVersion(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want bool
	}{
		{"bare release", "0.1.13", true},
		{"v-prefixed release", "v0.1.13", true},
		{"surrounding whitespace", "  v0.1.13  ", true},
		{"dev describe", "v0.2.15-235-gdaf0e935", false},
		{"dirty dev describe", "v0.2.15-235-gdaf0e935-dirty", false},
		{"empty", "", false},
		{"two components", "0.1", false},
		{"four components", "0.1.2.3", false},
		{"non-numeric", "1.0.x", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := IsReleaseVersion(tt.in); got != tt.want {
				t.Fatalf("IsReleaseVersion(%q) = %v, want %v", tt.in, got, tt.want)
			}
		})
	}
}

func TestIsNewerVersion(t *testing.T) {
	tests := []struct {
		name            string
		latest, current string
		want            bool
	}{
		{"patch bump", "v0.1.14", "v0.1.13", true},
		{"minor bump", "v0.2.0", "v0.1.99", true},
		{"major bump", "v1.0.0", "v0.99.99", true},
		{"same version", "v0.1.13", "v0.1.13", false},
		{"older latest", "v0.1.12", "v0.1.13", false},
		{"mixed v prefix", "0.1.14", "v0.1.13", true},
		{"current is dev describe → unparseable → false", "v0.1.14", "v0.1.13-5-gabcdef0", false},
		{"latest is dev describe → unparseable → false", "v0.1.14-1-gabcdef0", "v0.1.13", false},
		{"latest unparseable → false", "garbage", "v0.1.13", false},
		{"current unparseable → false", "v0.1.14", "garbage", false},
		{"empty latest", "", "v0.1.13", false},
		{"empty current", "v0.1.14", "", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := IsNewerVersion(tt.latest, tt.current); got != tt.want {
				t.Fatalf("IsNewerVersion(%q, %q) = %v, want %v", tt.latest, tt.current, got, tt.want)
			}
		})
	}
}

func TestFindChecksumManifestAsset(t *testing.T) {
	t.Run("finds checksums.txt among assets", func(t *testing.T) {
		assets := []GitHubReleaseAsset{
			{Name: "patchbay-cli-1.2.3-darwin-arm64.tar.gz"},
			{Name: "checksums.txt", BrowserDownloadURL: "https://example/checksums.txt"},
			{Name: "patchbay-cli-1.2.3-linux-amd64.tar.gz"},
		}
		got, err := findChecksumManifestAsset(assets)
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got.Name != "checksums.txt" || got.BrowserDownloadURL != "https://example/checksums.txt" {
			t.Fatalf("got %+v", got)
		}
	})

	t.Run("returns error when manifest missing", func(t *testing.T) {
		_, err := findChecksumManifestAsset([]GitHubReleaseAsset{
			{Name: "patchbay-cli-1.2.3-darwin-arm64.tar.gz"},
		})
		if err == nil {
			t.Fatal("expected error when checksums.txt is absent")
		}
	})
}

func TestParseChecksumManifest(t *testing.T) {
	manifest := []byte(strings.Join([]string{
		"# generated by GoReleaser",
		"",
		"aaaa1111  patchbay-cli-1.2.3-darwin-arm64.tar.gz",
		"bbbb2222  patchbay-cli-1.2.3-darwin-amd64.tar.gz",
		"cccc3333\tmulti-tab-separator.tar.gz",
		"DDDD4444  patchbay_linux_amd64.tar.gz",
	}, "\n"))

	t.Run("returns lowercase sha for matched entry", func(t *testing.T) {
		got, err := parseChecksumManifest(manifest, "patchbay-cli-1.2.3-darwin-arm64.tar.gz")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got != "aaaa1111" {
			t.Fatalf("sha = %q, want aaaa1111", got)
		}
	})

	t.Run("matches a tab-separated entry", func(t *testing.T) {
		got, err := parseChecksumManifest(manifest, "multi-tab-separator.tar.gz")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got != "cccc3333" {
			t.Fatalf("sha = %q, want cccc3333", got)
		}
	})

	t.Run("downcases an uppercase entry", func(t *testing.T) {
		got, err := parseChecksumManifest(manifest, "patchbay_linux_amd64.tar.gz")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if got != "dddd4444" {
			t.Fatalf("sha = %q, want dddd4444", got)
		}
	})

	t.Run("returns error when asset is absent", func(t *testing.T) {
		_, err := parseChecksumManifest(manifest, "not-in-manifest.tar.gz")
		if err == nil {
			t.Fatal("expected error for missing asset")
		}
	})

	t.Run("skips blank lines and comments", func(t *testing.T) {
		// If parsing broke on blank/comment lines we'd never reach the
		// matching entry below them.
		if _, err := parseChecksumManifest(manifest, "patchbay-cli-1.2.3-darwin-arm64.tar.gz"); err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
	})
}

func TestVerifyAssetSHA256(t *testing.T) {
	data := []byte("hello patchbay")
	sum := sha256.Sum256(data)
	good := hex.EncodeToString(sum[:])

	t.Run("accepts matching sha", func(t *testing.T) {
		if err := verifyAssetSHA256(data, good, "asset.tar.gz"); err != nil {
			t.Fatalf("expected ok, got %v", err)
		}
	})

	t.Run("accepts uppercase expected hex", func(t *testing.T) {
		if err := verifyAssetSHA256(data, strings.ToUpper(good), "asset.tar.gz"); err != nil {
			t.Fatalf("expected ok with uppercase expected, got %v", err)
		}
	})

	t.Run("rejects mismatched sha", func(t *testing.T) {
		err := verifyAssetSHA256([]byte("tampered"), good, "asset.tar.gz")
		if err == nil {
			t.Fatal("expected mismatch error")
		}
		if !strings.Contains(err.Error(), "asset.tar.gz") {
			t.Fatalf("error should name the asset: %v", err)
		}
	})

	t.Run("rejects empty expected", func(t *testing.T) {
		if err := verifyAssetSHA256(data, "", "asset.tar.gz"); err == nil {
			t.Fatal("expected error for empty expected sha")
		}
	})
}

func TestUpdateDownloadTimeoutOrDefault(t *testing.T) {
	tests := []struct {
		name    string
		timeout time.Duration
		want    time.Duration
	}{
		{
			name:    "uses default for zero",
			timeout: 0,
			want:    DefaultUpdateDownloadTimeout,
		},
		{
			name:    "uses default for negative",
			timeout: -1 * time.Second,
			want:    DefaultUpdateDownloadTimeout,
		},
		{
			name:    "keeps explicit timeout",
			timeout: 10 * time.Minute,
			want:    10 * time.Minute,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := updateDownloadTimeoutOrDefault(tt.timeout)
			if got != tt.want {
				t.Fatalf("timeout = %s, want %s", got, tt.want)
			}
		})
	}
}
