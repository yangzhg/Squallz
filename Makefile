SHELL := /bin/bash

ROOT := $(CURDIR)
FRONTEND := $(ROOT)/frontend
UNAME_S := $(shell uname -s 2>/dev/null || echo unknown)
NATIVE_OS := $(if $(filter Windows_NT,$(OS)),windows,$(if $(filter Darwin,$(UNAME_S)),macos,$(if $(filter Linux,$(UNAME_S)),linux,unknown)))
MACOS_APP_DEBUG := $(ROOT)/target/debug/bundle/macos/Squallz.app
MACOS_APP_RELEASE := $(ROOT)/target/release/bundle/macos/Squallz.app
SMOKE_APP ?= $(MACOS_APP_DEBUG)
PACKAGE_APP ?= $(MACOS_APP_RELEASE)
POWERSHELL ?= powershell

.PHONY: help
help:
	@echo "Squallz make targets"
	@echo "Host: $(NATIVE_OS)"
	@echo
	@echo "Setup:"
	@echo "  make install              Install frontend dependencies with npm ci"
	@echo "  make precommit-install    Install git pre-commit hook"
	@echo
	@echo "Checks:"
	@echo "  make fmt                  Format Rust workspace"
	@echo "  make fmt-check            Check Rust formatting"
	@echo "  make rust-check           cargo check --workspace"
	@echo "  make frontend-check       Svelte/style checks"
	@echo "  make check                Formatting, Rust check, frontend check"
	@echo "  make precommit            Run pre-commit on all files"
	@echo
	@echo "Tests:"
	@echo "  make test-rust            Rust tests except GUI crate"
	@echo "  make test-gui             GUI lib/bin tests"
	@echo "  make test                 Rust + GUI tests"
	@echo
	@echo "Build:"
	@echo "  make build                cargo build --workspace"
	@echo "  make build-release        cargo build --workspace --release"
	@echo "  make frontend-build       Vite production build"
	@echo
	@echo "App packaging:"
	@echo "  make app                  Build debug Tauri app"
	@echo "  make app-debug            Build debug Tauri app for current OS"
	@echo "  make app-release          Build release Tauri app and bundles for current OS"
	@echo "  make package              Alias for app-release"
	@echo "  make app-macos            Build macOS package on macOS"
	@echo "  make app-linux            Build Linux package on Linux"
	@echo "  make app-windows          Build Windows package on Windows"
	@echo "  make package-macos        Alias for app-macos"
	@echo "  make package-linux        Alias for app-linux"
	@echo "  make package-windows      Alias for app-windows"
	@echo
	@echo "Smoke:"
	@echo "  make smoke-native         Run the native smoke for the current OS"
	@echo "  make smoke-macos          Launch real macOS app smoke"
	@echo "  make smoke-linux          Run Linux Secret Service smoke on Linux"
	@echo "  make smoke-windows        Run Windows Credential Manager smoke on Windows"
	@echo
	@echo "Cleanup:"
	@echo "  make clean                Remove Rust and frontend build output"
	@echo
	@echo "Variables:"
	@echo "  SMOKE_APP=/path/App.app make smoke-macos"
	@echo "  POWERSHELL=pwsh make smoke-windows"

.PHONY: require-macos require-linux require-windows
require-macos:
	@if [[ "$(NATIVE_OS)" != "macos" ]]; then \
		echo "This target must run on macOS. Current host: $(NATIVE_OS)"; \
		exit 2; \
	fi

require-linux:
	@if [[ "$(NATIVE_OS)" != "linux" ]]; then \
		echo "This target must run on Linux. Current host: $(NATIVE_OS)"; \
		exit 2; \
	fi

require-windows:
	@if [[ "$(NATIVE_OS)" != "windows" ]]; then \
		echo "This target must run on Windows. Current host: $(NATIVE_OS)"; \
		exit 2; \
	fi

.PHONY: install
install:
	cd "$(FRONTEND)" && npm ci

.PHONY: precommit-install
precommit-install:
	pre-commit install

.PHONY: fmt
fmt:
	cargo fmt --all

.PHONY: fmt-check
fmt-check:
	cargo fmt --all -- --check

.PHONY: rust-check
rust-check:
	cargo check --workspace

.PHONY: frontend-check
frontend-check:
	cd "$(FRONTEND)" && npm run check

.PHONY: check
check: fmt-check rust-check frontend-check

.PHONY: precommit
precommit:
	pre-commit run --all-files

.PHONY: test-rust
test-rust:
	cargo test --workspace --exclude squallz-gui

.PHONY: test-gui
test-gui:
	cargo test -p squallz-gui --lib --bins

.PHONY: test
test: test-rust test-gui

.PHONY: build
build:
	cargo build --workspace

.PHONY: build-release
build-release:
	cargo build --workspace --release

.PHONY: frontend-build
frontend-build:
	cd "$(FRONTEND)" && npm run build

.PHONY: dev
dev:
	cd "$(FRONTEND)" && npm run tauri:dev

.PHONY: app app-debug
app: app-debug

app-debug:
	cd "$(FRONTEND)" && npm run tauri -- build --debug

.PHONY: app-release package
app-release:
	cd "$(FRONTEND)" && npm run tauri -- build

package: app-release

.PHONY: app-macos app-linux app-windows package-macos package-linux package-windows
app-macos: require-macos app-release

app-linux: require-linux app-release

app-windows: require-windows app-release

package-macos: app-macos

package-linux: app-linux

package-windows: app-windows

.PHONY: smoke-native smoke-app smoke-macos smoke-linux smoke-windows
smoke-native:
	@case "$(NATIVE_OS)" in \
		macos) $(MAKE) smoke-macos ;; \
		linux) $(MAKE) smoke-linux ;; \
		windows) $(MAKE) smoke-windows ;; \
		*) echo "No native smoke is configured for host: $(NATIVE_OS)"; exit 2 ;; \
	esac

smoke-app: smoke-macos

smoke-macos: require-macos
	scripts/macos_app_smoke.sh "$(SMOKE_APP)"

smoke-linux: require-linux
	scripts/linux_secret_service_smoke.sh

smoke-windows: require-windows
	"$(POWERSHELL)" -ExecutionPolicy Bypass -File scripts/windows_credential_manager_smoke.ps1

.PHONY: clean
clean:
	cargo clean
	rm -rf "$(FRONTEND)/dist"
