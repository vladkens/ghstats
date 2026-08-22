.PHONY: prepare check test build update docker-build backup

prepare:
	cargo +nightly fmt
	cargo clippy --fix --locked --allow-dirty -- -D warnings
	cargo check --release --locked

check:
	cargo +nightly fmt --check
	cargo clippy --locked -- -D warnings
	cargo check --release --locked

test:
	cargo test --locked

build:
	cargo build --release --locked
	ls -lh target/release/$(shell basename $(CURDIR))

update:
	cargo upgrade -i

docker-build:
	docker build -t ghstats:latest .
	docker images -q ghstats:latest | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

# --- Deploy ---

HOST=srv

backup:
	mkdir -p data
	scp $(HOST):/root/data/ghstats.db data/backup-$(shell date -u +"%Y%m%d_%H%M").db
	cp $$(ls -1t data/*.db | head -n 1) data/backup.db
