.PHONY: prepare test watch build update docker-build backup

prepare:
	cargo fmt
	cargo clippy --all-targets --all-features --locked -- -D warnings
	cargo check --release --locked

test:
	cargo test --locked

watch:
	watchexec --restart --watch src --watch assets --exts rs,css,js,svg -- cargo run --locked

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
