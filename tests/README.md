# Integration / E2E tests

## binary-store-e2e.sh

Tests cafe-binary-store HTTP API end-to-end: write, read, range, delete, 404.

Requires release binaries built first:

```
cargo build --release
```

Then run:

```
./tests/binary-store-e2e.sh
```

Starts cafe-bus + cafe-binary-store on ephemeral ports, runs curl-based tests,
cleans up on exit.
