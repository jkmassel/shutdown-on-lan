
test:
	cargo test -- --test-threads 1

lint:
	cargo fmt && cargo clippy

linux_lint:
	docker run -e CARGO_HOME=/app/.cargo -it --rm -v  $(shell pwd):/app --workdir /app rust:1.62 rustup component add rustfmt && cargo fmt && cargo clippy

linux_test:
	docker run -e CARGO_HOME=/app/.cargo -it --rm -v  $(shell pwd):/app --workdir /app -e RUST_BACKTRACE=1 rust:1.62 cargo test -- --test-threads 1

