# ReelStudio: build, test and package the Mac app (docs/specs/mac-app.md).
#   make -C macos test    swift test (ReelCoreTests)
#   make -C macos app     dist/ReelStudio.app with reelctl inside, ad-hoc signed
#   make -C macos dmg     dist/ReelStudio-<version>.dmg
# Needs Xcode 16 (or its command line tools), cargo, and Homebrew ffmpeg + pkg-config.

ROOT := $(abspath ..)
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(ROOT)/Cargo.toml | head -1)
DIST := $(CURDIR)/dist
APP := $(DIST)/ReelStudio.app

.PHONY: all test build reelctl app dmg clean

all: test dmg

test:
	swift test

build:
	swift build -c release --arch arm64

reelctl:
	cd $(ROOT) && cargo build --release -p reelctl

app: build reelctl
	VERSION=$(VERSION) ROOT=$(ROOT) DIST=$(DIST) ./scripts/make-app.sh

dmg: app
	VERSION=$(VERSION) DIST=$(DIST) ./scripts/make-dmg.sh

clean:
	rm -rf .build $(DIST)
