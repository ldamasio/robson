# Makefile for Robson CLI

VERSION := 0.2.0
BUILD_TIME := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)
LDFLAGS := -X main.Version=$(VERSION) -X main.BuildTime=$(BUILD_TIME)

.PHONY: all build clean test smoke-test install help

# Default target
all: build

# Build the Go binary
build:
	@echo "Building robson-go..."
	go build -o robson-go -ldflags="$(LDFLAGS)" .
	@echo "✓ Built robson-go"

# Build both C router and Go binary
build-all: build
	@echo "Building C router..."
	cd .. && gcc -o robson main.c
	@echo "✓ Built robson (C router)"
	@echo ""
	@echo "Build complete!"
	@echo "  - robson (C router): ../robson"
	@echo "  - robson-go (Go impl): ./robson-go"

# Clean build artifacts
clean:
	@echo "Cleaning..."
	rm -f robson-go
	rm -f ../robson
	@echo "✓ Clean complete"

# Run Go tests
test:
	@echo "Running tests..."
	go test -v ./...

# Run smoke tests
smoke-test: build
	@echo "=== Robson CLI Smoke Tests ==="
	@echo ""
	@echo "Test 1: Help command"
	@./robson-go help > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"
	@echo ""
	@echo "Test 2: Say command"
	@OUTPUT=$$(./robson-go say "test"); \
	echo "Output: $$OUTPUT"; \
	echo "$$OUTPUT" | grep -q "Robson says: test" && echo "✓ PASSED" || echo "✗ FAILED"
	@echo ""
	@echo "Test 3: JSON output"
	@./robson-go help --json | jq . > /dev/null 2>&1 && echo "✓ PASSED" || echo "✗ FAILED (jq required)"
	@echo ""
	@echo "Test 4: Plan creation"
	@./robson-go plan buy BTCUSDT 0.001 > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"
	@echo ""
	@echo "Test 5: Version"
	@./robson-go version > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"
	@echo ""
	@echo "=== All smoke tests completed ==="

# Install to system (requires sudo)
install: build
	@echo "Installing to /usr/local/bin..."
	sudo cp robson-go /usr/local/bin/
	cd .. && sudo cp robson /usr/local/bin/
	@echo "✓ Installed successfully"
	@echo ""
	@echo "Try: robson help"

# Install to local bin (no sudo required)
install-local: build-all
	@echo "Installing to ~/bin..."
	mkdir -p ~/bin
	cp robson-go ~/bin/
	cp ../robson ~/bin/
	@echo "✓ Installed to ~/bin"
	@echo ""
	@echo "Make sure ~/bin is in your PATH:"
	@echo "  export PATH=\$$PATH:~/bin"

# Download dependencies
deps:
	@echo "Downloading dependencies..."
	go mod download
	@echo "✓ Dependencies downloaded"

# Display help
help:
	@echo "Robson CLI Makefile"
	@echo ""
	@echo "Targets:"
	@echo "  make build         - Build robson-go binary"
	@echo "  make build-all     - Build both C router and Go binary"
	@echo "  make clean         - Remove build artifacts"
	@echo "  make test          - Run Go tests"
	@echo "  make smoke-test    - Run smoke tests"
	@echo "  make install       - Install to /usr/local/bin (requires sudo)"
	@echo "  make install-local - Install to ~/bin (no sudo)"
	@echo "  make deps          - Download Go dependencies"
	@echo "  make help          - Show this help"
