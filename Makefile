PREFIX     ?= /opt/ttstack
CARGO      ?= cargo
CARGO_FLAG ?= --locked

.PHONY: all build release test lint fmt fmt-check doc clean \
        install uninstall deploy-agent deploy-ctl deploy deploy-dist help

all: fmt lint build test

## Build debug binaries
build:
	$(CARGO) build $(CARGO_FLAG)

## Build optimized release binaries
release:
	$(CARGO) build --release $(CARGO_FLAG)

## Run all tests
test:
	$(CARGO) test $(CARGO_FLAG)

## Run clippy linter (treat warnings as errors)
lint:
	$(CARGO) clippy $(CARGO_FLAG) -- -D warnings

## Format code
fmt:
	$(CARGO) fmt

## Check formatting without modifying
fmt-check:
	$(CARGO) fmt -- --check

## Generate Rust source API documentation (HTTP API: docs/rest-api.md)
doc:
	$(CARGO) doc --no-deps --document-private-items $(CARGO_FLAG)

## Remove build artifacts
clean:
	$(CARGO) clean

## Install release binaries to PREFIX/bin (no systemd setup)
install: release
	@mkdir -p $(PREFIX)/bin
	install -m 755 target/release/tt       $(PREFIX)/bin/tt
	install -m 755 target/release/tt-ctl   $(PREFIX)/bin/tt-ctl
	install -m 755 target/release/tt-agent $(PREFIX)/bin/tt-agent
	@echo "Installed to $(PREFIX)/bin/"

## Remove installed binaries
uninstall:
	rm -f $(PREFIX)/bin/tt $(PREFIX)/bin/tt-ctl $(PREFIX)/bin/tt-agent

## Deploy tt-agent on this host (requires root, idempotent)
deploy-agent: release
	sudo target/release/tt deploy agent

## Deploy tt-ctl (controller + web UI) on this host (requires root, idempotent)
deploy-ctl: release
	sudo target/release/tt deploy ctl

## Deploy both agent and controller on this host
deploy: release
	sudo target/release/tt deploy all

## Distributed deploy to all hosts in deploy.toml (via SSH)
deploy-dist: release
	target/release/tt deploy dist deploy.toml

## Show available targets
help:
	@echo "TTstack build targets:"
	@echo ""
	@echo "  Development:"
	@echo "    make build        Debug build"
	@echo "    make release      Optimized release build"
	@echo "    make test         Run all tests"
	@echo "    make lint         Run clippy"
	@echo "    make fmt          Format code"
	@echo "    make doc          Generate docs"
	@echo "    make clean        Remove build artifacts"
	@echo ""
	@echo "  Installation:"
	@echo "    make install      Copy binaries to $(PREFIX)/bin/"
	@echo "    make uninstall    Remove installed binaries"
	@echo ""
	@echo "  Deployment (local, requires root, idempotent):"
	@echo "    make deploy-agent Deploy tt-agent on this host"
	@echo "    make deploy-ctl   Deploy tt-ctl + web UI on this host"
	@echo "    make deploy       Deploy both on this host"
	@echo ""
	@echo "  Deployment (distributed, via SSH):"
	@echo "    make deploy-dist  Deploy to fleet (reads deploy.toml)"
	@echo ""
	@echo "  Images (auto-generate guest images):"
	@echo "    tt image recipes  List available image recipes"
	@echo "    tt image create <name> [--image-dir DIR]"
	@echo "    tt image create all --engine docker"
