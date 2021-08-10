.phony: all test

all:
	cargo fmt
	cargo build
	cargo clippy
	cargo build --examples

test:
	cargo test --features std

clean:
	cargo clean

riscv:
	cargo build --target riscv32imac-unknown-none-elf