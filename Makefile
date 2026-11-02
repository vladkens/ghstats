.PHONY: prepare check test build update vendor docker-build backup

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

# Frontend libs are committed to assets/vendor and embedded into the binary.
# To upgrade one, bump its version below and run `make vendor`.
PICO_VERSION=2.0.6
CHARTJS_VERSION=4.4.9
LUXON_VERSION=3.5.0
CHARTJS_LUXON_VERSION=1.3.1
HTMX_VERSION=2.0.11

vendor:
	mkdir -p assets/vendor
	curl -fsSL -o assets/vendor/pico.min.css https://unpkg.com/@picocss/pico@$(PICO_VERSION)/css/pico.min.css
	curl -fsSL -o assets/vendor/chart.umd.js https://unpkg.com/chart.js@$(CHARTJS_VERSION)/dist/chart.umd.js
	curl -fsSL -o assets/vendor/luxon.min.js https://unpkg.com/luxon@$(LUXON_VERSION)/build/global/luxon.min.js
	curl -fsSL -o assets/vendor/chartjs-adapter-luxon.umd.min.js https://unpkg.com/chartjs-adapter-luxon@$(CHARTJS_LUXON_VERSION)/dist/chartjs-adapter-luxon.umd.min.js
	curl -fsSL -o assets/vendor/htmx.min.js https://unpkg.com/htmx.org@$(HTMX_VERSION)/dist/htmx.min.js
	ls -lh assets/vendor

docker-build:
	docker build -t ghstats:latest .
	docker images -q ghstats:latest | xargs docker inspect -f '{{.Size}}' | xargs numfmt --to=iec

# --- Deploy ---

HOST=srv

backup:
	mkdir -p data
	scp $(HOST):/root/data/ghstats.db data/backup-$(shell date -u +"%Y%m%d_%H%M").db
	cp $$(ls -1t data/*.db | head -n 1) data/backup.db
