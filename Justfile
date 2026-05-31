# ObservableCAFE monorepo task runner
# Install: https://github.com/casey/just

default:
    @just --list

# Build everything
build:
    cargo build --workspace
    cd cafe-telegram && go build -o cafe-telegram .
    cd cafe-web && npm install && npm run build

# Build only Rust crates
build-rust:
    cargo build --workspace

# Build debug and start all services
dev:
    just build-rust
    process-compose up

# Stop all services
down:
    process-compose down

# Run all tests
test:
    cargo test --workspace
    cd cafe-telegram && go test ./...

# Format all code
fmt:
    cargo fmt --all
    cd cafe-telegram && gofmt -w .

# Lint all code
lint:
    cargo clippy --workspace -- -D warnings
    cd cafe-telegram && go vet ./...

# Clean all build artifacts
clean:
    cargo clean
    rm -f cafe-telegram/cafe-telegram
    rm -rf cafe-web/dist cafe-web/node_modules
