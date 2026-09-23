
test:
	cargo test -- --test-threads 1

lint:
	cargo fmt && cargo clippy

linux_lint:
	docker run -e CARGO_HOME=/app/.cargo -it --rm -v  $(shell pwd):/app --workdir /app rust:1 sh -c "rustup component add rustfmt clippy && cargo fmt && cargo clippy"

linux_test:
	docker run -e CARGO_HOME=/app/.cargo -it --rm -v  $(shell pwd):/app --workdir /app -e RUST_BACKTRACE=1 rust:1 cargo test -- --test-threads 1

